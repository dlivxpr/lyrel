//! [`Transport`] over HTTPS, with the same stack and proxy resolution the
//! installer and the update check use.
//!
//! Blocking — every call runs on a worker, never on the UI thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::api::{ApiError, Reply, Transport, classify, link_has_next};
use super::token::Token;

const API: &str = "https://api.github.com";

/// Interactive reads: long enough for a slow link, short enough that a dead
/// one is reported rather than sat on.
const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A pull request's file list with every patch inlined is the largest answer
/// the panel asks for; GitHub itself caps a patch well under this.
const MAX_BODY: u64 = 32 * 1024 * 1024;

/// Past this many remembered answers the cache starts over.
const ETAG_CAP: usize = 512;

pub struct HttpTransport {
    agent: ureq::Agent,
    token: Option<Token>,
    etags: Etags,
    // Only loopback HTTP fixtures can replace the production API authority.
    #[cfg(test)]
    api: Option<String>,
}

/// The last answer to each plain GET, by ETag. Re-asking with `If-None-Match`
/// costs nothing when it comes back 304: a signed-in 304 is not counted
/// against the rate limit, which is what keeps the panel's revalidations and
/// polls from eating the hourly allowance it shares with `gh`.
#[derive(Default)]
struct Etags(Mutex<HashMap<String, Arc<(String, Reply)>>>);

impl Etags {
    /// The stored tag and the answer it stands for, read together: a 304
    /// replays the answer that went with the tag that was sent, even if the
    /// entry has since been replaced or the cache has started over.
    fn get(&self, path: &str) -> Option<Arc<(String, Reply)>> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(path).cloned()
    }

    fn store(&self, path: &str, tag: String, reply: &Reply) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // A wholesale clear rather than an LRU: the panel's working set sits
        // far below the cap.
        if map.len() >= ETAG_CAP && !map.contains_key(path) {
            map.clear();
        }
        map.insert(path.to_string(), Arc::new((tag, reply.clone())));
    }
}

impl HttpTransport {
    /// `manual_proxy` is the `http_proxy` setting, as for lyrel's other
    /// downloads.
    pub fn new(token: Option<Token>, manual_proxy: Option<&str>) -> HttpTransport {
        let mut builder = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            // Renamed repositories redirect to /repositories/{id}. Preserve
            // their token only on the same host, never on an HTTPS downgrade.
            .redirect_auth_headers(ureq::config::RedirectAuthHeaders::SameHost)
            // 4xx/5xx come back as responses, so their headers (the rate
            // limit's reset time) can be read.
            .http_status_as_error(false)
            .user_agent(concat!("lyrel/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = crate::daemon::install::proxy::resolve(API, manual_proxy) {
            builder = builder.proxy(Some(proxy));
        }
        // Unit fixtures never use the developer's proxy configuration.
        #[cfg(test)]
        {
            builder = builder.proxy(None);
        }
        HttpTransport {
            agent: builder.build().into(),
            token,
            etags: Etags::default(),
            #[cfg(test)]
            api: None,
        }
    }
}

impl Transport for HttpTransport {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        self.request(path, "application/vnd.github+json")
    }

    fn get_full(&self, path: &str) -> Result<Reply, ApiError> {
        self.request(path, "application/vnd.github.full+json")
    }

    fn authenticated(&self) -> bool {
        self.token.is_some()
    }
}

impl HttpTransport {
    fn request(&self, path: &str, accept: &str) -> Result<Reply, ApiError> {
        // The caller supplies an API path, never another authority or a URL.
        // Without this check, `@host/path` would replace api.github.com's
        // authority after concatenation and receive the Authorization header.
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['\\', '#'])
            || path.chars().any(char::is_control)
        {
            return Err(ApiError::Decode("Expected a GitHub API path".into()));
        }
        #[cfg(not(test))]
        let api = API;
        #[cfg(test)]
        let api = self.api.as_deref().unwrap_or(API);
        let mut request = self
            .agent
            .get(format!("{api}{path}"))
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {}", token.expose()));
        }
        // Only a plain GET is revalidated: `get_full` carries signed
        // attachment URLs that expire, so its old body must not be replayed.
        let cacheable = accept == "application/vnd.github+json";
        let known = cacheable.then(|| self.etags.get(path)).flatten();
        if let Some(known) = &known {
            request = request.header("If-None-Match", &known.0);
        }
        // ureq's error text names the URL and the transport failure, never
        // the request headers, so it is safe to show.
        let mut response = request
            .call()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        if status == 304
            && let Some(known) = known
        {
            return Ok(known.1.clone());
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        classify(status, &header, &body)?;
        let reply = Reply {
            has_next: header("link").is_some_and(|l| link_has_next(&l)),
            body,
        };
        if cacheable && let Some(tag) = header("etag") {
            self.etags.store(path, tag, &reply);
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::super::api::{ListPage, ListQuery, list};
    use super::super::model::{Kind, StateFilter};
    use super::super::remote::RepoSlug;
    use super::*;

    #[test]
    fn malformed_paths_cannot_redirect_a_token_to_another_authority() {
        let http = HttpTransport::new(Token::new("synthetic-http-secret"), None);
        for path in [
            "@example.invalid/private",
            ".example.invalid/private",
            "https://example.invalid/private",
            "//example.invalid/private",
            "/repos/acme/tools#fragment",
            "/repos/acme/\\tools",
            "/repos/acme/tools\r\nAuthorization: injected",
        ] {
            for result in [http.get(path), http.get_full(path)] {
                assert!(matches!(result, Err(ApiError::Decode(_))));
            }
        }
    }

    /// The real HTTP transport and list decoder read a private repository
    /// through its old slug. The destination serves data only with the token;
    /// a missing token produces GitHub's private-repository 404, not an echo.
    fn renamed_repository(redirect_host: &str, status: u16) -> Result<ListPage, ApiError> {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::time::Instant;

        fn receive(listener: &TcpListener) -> (TcpStream, String) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "loopback request timed out");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("loopback accept failed: {error}"),
                }
            };
            // Windows inherits the listener's nonblocking mode on accept.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                assert!(bytes.len() < 16 * 1024, "oversized request headers");
            }
            (stream, String::from_utf8(bytes).unwrap())
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let location = format!(
            "http://{redirect_host}:{}/repositories/123/issues?state=open&sort=updated&direction=desc&per_page=50&page=2",
            address.port()
        );
        let same_host = redirect_host == "127.0.0.1";
        let server = std::thread::spawn(move || {
            let (mut first, initial) = receive(&listener);
            assert!(initial.starts_with("GET /repos/acme/old/issues?state=open&sort=updated&direction=desc&per_page=50&page=2 HTTP/1.1\r\n"));
            let expected = format!("{} {}", "Bearer", "synthetic-http-secret");
            let authorized = |request: &str| {
                request
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .any(|(name, value)| {
                        name.eq_ignore_ascii_case("authorization") && value.trim() == expected
                    })
            };
            assert!(
                authorized(&initial),
                "the original private request needs authentication"
            );
            write!(first, "HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(first);
            let (mut second, redirected) = receive(&listener);
            assert!(redirected.starts_with("GET /repositories/123/issues?state=open&sort=updated&direction=desc&per_page=50&page=2 HTTP/1.1\r\n"));
            assert!(
                redirected
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("accept: application/vnd.github+json"))
            );
            assert!(
                redirected
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("x-github-api-version: 2022-11-28"))
            );
            if !same_host {
                assert!(!redirected.to_ascii_lowercase().contains("authorization:"));
                assert!(!redirected.contains("synthetic-http-secret"));
            }
            let (status, body) = if authorized(&redirected) {
                (
                    200,
                    r#"[{"number":42,"title":"Private renamed repository","state":"open","html_url":"https://github.com/acme/new/issues/42"}]"#,
                )
            } else {
                (404, r#"{"message":"Not Found"}"#)
            };
            write!(second, "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nContent-Type: application/json\r\nLink: <https://api.github.com/repositories/123/issues?page=3>; rel=\"next\"\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let mut http = HttpTransport::new(Token::new("synthetic-http-secret"), None);
        http.api = Some(format!("http://{address}"));
        let result = list(
            &http,
            &ListQuery {
                slug: RepoSlug {
                    owner: "acme".into(),
                    name: "old".into(),
                },
                kind: Kind::Issues,
                state: StateFilter::Open,
                label: None,
            },
            2,
        );
        server.join().unwrap();
        result
    }

    #[test]
    fn a_renamed_private_repository_returns_its_items_after_same_host_redirects() {
        for status in [301, 302, 307, 308] {
            let page = renamed_repository("127.0.0.1", status).unwrap();
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].number, 42);
            assert_eq!(page.items[0].title, "Private renamed repository");
            assert_eq!(page.next_page, Some(3));
        }
    }

    #[test]
    fn a_different_host_cannot_read_the_private_repository_with_our_token() {
        assert_eq!(
            renamed_repository("localhost", 301),
            Err(ApiError::NotFound)
        );
    }

    fn reply(body: &str) -> Reply {
        Reply {
            body: body.as_bytes().to_vec(),
            has_next: true,
        }
    }

    #[test]
    fn a_stored_answer_is_replayed_by_its_tag() {
        let etags = Etags::default();
        assert!(etags.get("/a").is_none());
        etags.store("/a", "W/\"1\"".into(), &reply("one"));
        let first = etags.get("/a").unwrap();
        etags.store("/a", "W/\"2\"".into(), &reply("two"));
        let (tag, back) = &*etags.get("/a").unwrap();
        assert_eq!(tag, "W/\"2\"");
        assert_eq!((back.body.as_slice(), back.has_next), (&b"two"[..], true));
        // What was read before the replacement still pairs its own tag and
        // answer.
        assert_eq!(
            (first.0.as_str(), first.1.body.as_slice()),
            ("W/\"1\"", &b"one"[..])
        );
    }

    #[test]
    fn the_cache_starts_over_past_its_cap() {
        let etags = Etags::default();
        for i in 0..ETAG_CAP {
            etags.store(&format!("/{i}"), "t".into(), &reply(""));
        }
        etags.store("/0", "t2".into(), &reply(""));
        assert!(etags.get("/1").is_some());
        let held = etags.get("/1").unwrap();
        etags.store("/new", "t".into(), &reply(""));
        assert!(etags.get("/1").is_none());
        assert_eq!(etags.get("/new").map(|e| e.0.clone()).as_deref(), Some("t"));
        // An answer read before the clear can still be replayed for its 304.
        assert_eq!(held.0, "t");
    }
}
