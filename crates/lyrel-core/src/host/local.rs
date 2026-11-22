//! The full local Host: `lyrel-protocol`'s C-free [`LocalHost`] with this
//! crate's SQLite engine, whole-host metrics sampler and live agent scopes attached.
//!
//! [`new`] and [`shared`] are the constructors production entries must use:
//! the GUI's `HostRegistry`, the Windows/Unix daemon entries and
//! `lyrel-server --stdio` all serve remote clients that browse databases and
//! read whole-host metrics, so silently falling back to the bare base would
//! drop shipped capabilities. The bare base remains available for thin
//! clients (the CLI) as `lyrel_protocol::host::local::LocalHost::new`; both
//! shapes are the same `LocalHost` type and the same single `Host`
//! implementation — only the injected capabilities differ.

use std::sync::{Arc, LazyLock};

use lyrel_protocol::host::SharedHost;

pub use lyrel_protocol::host::local::LocalHost;

/// This machine's Host with its read-only SQLite browsing engine, whole-host
/// metrics sampler, and active pane scopes for agent history.
pub fn new() -> SharedHost {
    LocalHost::with_capabilities(
        Some(Arc::new(crate::host::sqlite::SqliteService::new())),
        Some(crate::host::metrics::sample_local),
        crate::core::agent_history::local_roots,
    )
}

/// The process-wide full local Host singleton. The daemon and GUI registry
/// share one so every local pane and control connection browses and samples
/// through the same engine.
pub fn shared() -> SharedHost {
    static LOCAL: LazyLock<SharedHost> = LazyLock::new(new);
    LOCAL.clone()
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use crate::host::conformance::Sandbox;
    use crate::host::sqlite::SqliteCancel;
    use crate::host::{Entry, SharedHost};

    struct TempSandbox(tempfile::TempDir);

    impl Sandbox for TempSandbox {
        fn path(&self) -> &Path {
            self.0.path()
        }

        fn symlink(&self, target: &Path, link: &Path) -> Option<io::Result<()>> {
            #[cfg(unix)]
            {
                Some(std::os::unix::fs::symlink(target, link))
            }
            #[cfg(not(unix))]
            {
                let _ = (target, link);
                None
            }
        }
    }

    /// The conformance suite runs against the full local Host, so the SQLite
    /// cases still exercise the real engine rather than a bare `Unsupported`.
    fn sandbox() -> (SharedHost, TempSandbox) {
        (super::new(), TempSandbox(tempfile::TempDir::new().unwrap()))
    }

    crate::host_conformance_suite!(local, sandbox);

    #[test]
    fn the_full_local_host_browses_sqlite_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE t(id INTEGER PRIMARY KEY, body TEXT);\
                 INSERT INTO t VALUES (1, 'hello');",
            )
            .unwrap();
        drop(connection);

        let host = super::new();
        let opened = host.sqlite_open(&path, &SqliteCancel::default()).unwrap();
        assert_eq!(opened.objects.len(), 1);
        assert_eq!(opened.objects[0].name, "t");
        let columns = host
            .sqlite_columns(opened.view, "t", &SqliteCancel::default())
            .unwrap();
        assert_eq!(columns.names, vec!["id", "body"]);
        let count = host
            .sqlite_count(
                opened.view,
                &crate::host::sqlite::SqliteQuery {
                    table: "t".into(),
                    columns: columns.names,
                    order: columns.order,
                    ..crate::host::sqlite::SqliteQuery::default()
                },
                &SqliteCancel::default(),
            )
            .unwrap();
        assert_eq!(count, 1);
        host.sqlite_close(opened.view);
    }

    #[test]
    fn the_full_local_host_samples_whole_host_metrics() {
        let host = super::new();
        let metrics = host
            .host_metrics()
            .expect("the full local host carries the platform metrics sampler");
        assert_eq!(
            metrics.instance(),
            lyrel_protocol::daemon::control::server_instance()
        );
        // The GPU slot rides the same per-host sample (#374): once a real
        // reading lands, it is either a validated percentage or an explicit
        // unavailable reason — never an unexplained hole.
        std::thread::sleep(Duration::from_millis(1_100));
        let metrics = host
            .host_metrics()
            .expect("the platform metrics sampler keeps answering");
        if let lyrel_protocol::host::metrics::MetricsState::Sample(sample) = metrics.state() {
            match (sample.gpu_percent(), sample.gpu_unavailable_reason()) {
                (Some(gpu), None) => assert!(
                    (0.0..=100.0).contains(&gpu),
                    "GPU utilization stays a validated percentage, got {gpu}"
                ),
                (None, Some(reason)) => assert!(!reason.is_empty()),
                other => panic!("the GPU reading and its reason disagree: {other:?}"),
            }
        }
    }

    #[test]
    fn gitignore_chain_scores_the_file_tree_fixture() {
        let (h, tmp) = sandbox();
        let root = tmp.path();
        h.create_dir(&root.join(".git"), false).unwrap();
        h.create_dir(&root.join("src"), false).unwrap();
        h.write_file(&root.join(".gitignore"), b"*.log\nbuild/\n")
            .unwrap();
        h.write_file(&root.join("src/.gitignore"), b"!keep.log\n")
            .unwrap();
        h.write_file(&root.join("drop.log"), b"").unwrap();
        h.write_file(&root.join("src/keep.log"), b"").unwrap();
        h.write_file(&root.join("src/main.rs"), b"").unwrap();

        let ignored = |entries: &[Entry], name: &str| {
            entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .ignored
        };
        let top = h.read_dir(root, Some(root)).unwrap();
        assert!(ignored(&top, "drop.log"));
        assert!(ignored(&top, ".git"));
        assert!(!ignored(&top, "src"));

        let nested = h.read_dir(&root.join("src"), Some(root)).unwrap();
        assert!(!ignored(&nested, "keep.log"), "whitelist un-ignores");
        assert!(!ignored(&nested, "main.rs"));

        let hits = h
            .search(&[root.to_path_buf()], "log", 200, 2000, false)
            .unwrap();
        let names: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["keep.log"]);
    }

    #[test]
    fn a_gitignore_edit_through_the_watcher_clears_the_cache() {
        let (h, tmp) = sandbox();
        let root = tmp.path().to_path_buf();
        h.write_file(&root.join(".gitignore"), b"*.log\n").unwrap();
        h.write_file(&root.join("a.log"), b"").unwrap();
        let listed = h.read_dir(&root, Some(&root)).unwrap();
        assert!(listed.iter().any(|e| e.name == "a.log" && e.ignored));

        let sub = h.watch(std::slice::from_ref(&root)).unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut cleared = false;
        while Instant::now() < deadline {
            h.write_file(&root.join(".gitignore"), b"# nothing\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(250));
            while sub.events().try_recv().is_ok() {}
            if h.read_dir(&root, Some(&root))
                .unwrap()
                .iter()
                .any(|e| e.name == "a.log" && !e.ignored)
            {
                cleared = true;
                break;
            }
        }
        assert!(
            cleared,
            "a `.gitignore` change seen by the watcher must drop the compiled matchers"
        );
    }
}
