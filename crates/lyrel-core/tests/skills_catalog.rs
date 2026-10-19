#![cfg(feature = "github")]

use lyrel_core::skills::catalog::GitHubSource;

#[test]
fn github_inputs_preserve_repository_ref_subpath_and_skill_selection() {
    for input in [
        "github:acme/toolkit/skills/pdf#release%2Fstable@pdf",
        "acme/toolkit/skills/pdf#release%2Fstable@pdf",
        "https://github.com/acme/toolkit/tree/release%2Fstable/skills/pdf#release%2Fstable@pdf",
    ] {
        let source = GitHubSource::parse(input).unwrap();
        assert_eq!(source.repository(), "acme/toolkit");
        assert_eq!(source.url(), "https://github.com/acme/toolkit.git");
        assert_eq!(source.reference.as_deref(), Some("release/stable"));
        assert_eq!(source.subpath.as_deref(), Some("skills/pdf"));
        assert_eq!(source.skill_filter.as_deref(), Some("pdf"));
    }
    let source = GitHubSource::parse("https://github.com/acme/toolkit.git").unwrap();
    assert_eq!(source.repository(), "acme/toolkit");
    assert!(source.reference.is_none());
    assert_eq!(
        GitHubSource::parse("acme/toolkit@pdf")
            .unwrap()
            .skill_filter
            .as_deref(),
        Some("pdf")
    );
}

#[test]
fn only_credential_free_github_repository_sources_and_safe_relative_paths_are_accepted() {
    for input in [
        "./skill",
        "C:/private/skill",
        "https://gitlab.com/acme/repo",
        "https://user:secret@github.com/acme/repo",
        "https://github.com/acme/repo?token=x",
        "https://github.com/acme/repo/archive/main.zip",
        "acme/repo/../outside",
        "acme/repo/%2e%2e/outside",
        "acme/repo/C%3A/outside",
        "acme/repo/dir%5c..%5coutside",
        "https://github.com/acme/repo/tree/",
        "acme/repo#",
        "acme/repo/%zz",
    ] {
        assert!(
            GitHubSource::parse(input).is_err(),
            "accepted unsafe source {input}"
        );
    }
}

use base64::{Engine as _, engine::general_purpose::STANDARD};
use lyrel_core::{
    core::github::{ApiError, Reply, Transport},
    skills::catalog::{discover, search},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Mutex};

const ROOT: &str = "1111111111111111111111111111111111111111";
const PDF_TREE: &str = "2222222222222222222222222222222222222222";
const PDF_BLOB: &str = "3333333333333333333333333333333333333333";
const OTHER_TREE: &str = "4444444444444444444444444444444444444444";
const OTHER_BLOB: &str = "5555555555555555555555555555555555555555";
const PDF: &str = "---\nname: pdf\ndescription: PDF tools\n---\n# PDF\n";
const OTHER: &str = "---\nname: other\ndescription: Other tools\n---\n# Other\n";

#[derive(Default)]
struct Http {
    responses: HashMap<String, Result<Reply, ApiError>>,
    calls: Mutex<Vec<String>>,
}
impl Http {
    fn json(&mut self, path: &str, value: Value) {
        self.responses.insert(
            path.into(),
            Ok(Reply {
                body: serde_json::to_vec(&value).unwrap(),
                has_next: false,
            }),
        );
    }
    fn blob(&mut self, sha: &str, contents: &[u8]) {
        self.json(&format!("/repos/acme/tools/git/blobs/{sha}"), json!({"sha":sha,"encoding":"base64","size":contents.len(),"content":STANDARD.encode(contents)}));
    }
    fn tree(&mut self, reference: &str, entries: Vec<Value>) {
        self.json(
            &format!("/repos/acme/tools/git/trees/{reference}?recursive=1"),
            json!({"sha":ROOT,"truncated":false,"tree":entries}),
        );
    }
}
impl Transport for Http {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        self.calls.lock().unwrap().push(path.into());
        self.responses
            .get(path)
            .cloned()
            .unwrap_or(Err(ApiError::NotFound))
    }
    fn authenticated(&self) -> bool {
        false
    }
}
fn dir(path: &str, sha: &str) -> Value {
    json!({"path":path,"mode":"040000","type":"tree","sha":sha})
}
fn file(path: &str, sha: &str, size: usize) -> Value {
    json!({"path":path,"mode":"100644","type":"blob","sha":sha,"size":size})
}
fn two_skills(reference: &str) -> Http {
    let mut http = Http::default();
    http.tree(
        reference,
        vec![
            dir("skills", ROOT),
            dir("skills/pdf", PDF_TREE),
            file("skills/pdf/SKILL.md", PDF_BLOB, PDF.len()),
            dir("skills/other", OTHER_TREE),
            file("skills/other/SKILL.md", OTHER_BLOB, OTHER.len()),
        ],
    );
    http.blob(PDF_BLOB, PDF.as_bytes());
    http.blob(OTHER_BLOB, OTHER.as_bytes());
    http
}

// The external authorization seam uses synthetic credentials only; no process
// environment, real gh executable, or user repository is consulted.
struct AuthorizedHttp {
    http: Http,
    token: Option<lyrel_core::core::github::Token>,
}
impl Transport for AuthorizedHttp {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        if self.token.as_ref().map(|token| token.expose()) != Some("synthetic-catalog-secret") {
            return Err(ApiError::NotFound);
        }
        self.http.get(path)
    }
    fn authenticated(&self) -> bool {
        self.token.is_some()
    }
}

#[test]
fn local_environment_or_gh_authorization_discovers_and_prepares_private_pinned_files() {
    use lyrel_core::core::github::token::{TokenEnv, TokenSource, resolve_token};

    for source in ["GH_TOKEN", "GITHUB_TOKEN", "gh"] {
        let vars = |name: &str| match name {
            "PATH" => Some(std::path::Path::new("synthetic-bin").to_string_lossy().into_owned()),
            name if name == source => Some("synthetic-catalog-secret".into()),
            _ => None,
        };
        let is_file = |_: &std::path::Path| true;
        let gh = |_: &std::path::Path, args: &[&str]| {
            assert_eq!(args, ["auth", "token", "--hostname", "github.com"]);
            Some("synthetic-catalog-secret\n".into())
        };
        let (token, token_source) = resolve_token(&TokenEnv {
            var: &vars,
            is_file: &is_file,
            gh: &gh,
        })
        .unwrap();
        assert_eq!(
            token_source,
            match source {
                "GH_TOKEN" => TokenSource::Env("GH_TOKEN"),
                "GITHUB_TOKEN" => TokenSource::Env("GITHUB_TOKEN"),
                _ => TokenSource::GhCli,
            }
        );
        let companion = "6666666666666666666666666666666666666666";
        let mut http = Http::default();
        http.tree(
            "release%2Fstable",
            vec![
                dir("skills", ROOT),
                dir("skills/pdf", PDF_TREE),
                file("skills/pdf/SKILL.md", PDF_BLOB, PDF.len()),
                file("skills/pdf/template.txt", companion, 8),
            ],
        );
        http.blob(PDF_BLOB, PDF.as_bytes());
        http.blob(companion, b"template");
        let http = AuthorizedHttp { http, token: Some(token) };
        let source = GitHubSource::parse("acme/tools/skills/pdf#release%2Fstable").unwrap();
        let catalog = discover(&http, &source).unwrap();
        let prepared = catalog.prepare_many(&http, &[0]).unwrap().remove(0);
        assert_eq!(catalog.tree_sha(), ROOT);
        assert_eq!(prepared.candidate.tree_hash, PDF_TREE);
        assert_eq!(prepared.candidate.source, source);
        assert_eq!(prepared.candidate.source.url(), "https://github.com/acme/tools.git");
        assert_eq!(prepared.files[0].path, std::path::Path::new("SKILL.md"));
        assert_eq!(prepared.files[0].contents, PDF.as_bytes());
        assert_eq!(prepared.files[1].path, std::path::Path::new("template.txt"));
        assert_eq!(prepared.files[1].contents, b"template");
        for output in [format!("{catalog:?}"), format!("{prepared:?}"), format!("{source:?}")] {
            assert!(!output.contains("synthetic-catalog-secret"));
        }
        assert!(http.http.calls.lock().unwrap().iter().all(|path| {
            !path.contains("synthetic-catalog-secret")
                && (path.contains("/git/trees/release%2Fstable?") || path.contains("/git/blobs/"))
        }));
    }
}

#[test]
fn private_tree_document_and_companion_failures_keep_the_original_api_classification() {
    use lyrel_core::skills::catalog::CatalogError;

    let failures = [
        ApiError::Unauthorized,
        ApiError::Forbidden("Resource protected by organization SAML enforcement".into()),
        ApiError::NotFound,
        ApiError::RateLimited { reset: Some(1_800_000_000) },
        ApiError::Network("connection timed out".into()),
    ];
    for failure in failures {
        for failed_stage in ["tree", "document", "companion"] {
            let mut http = Http::default();
            http.tree(
                "HEAD",
                vec![
                    file("SKILL.md", PDF_BLOB, PDF.len()),
                    file("template.txt", OTHER_BLOB, 8),
                ],
            );
            http.blob(PDF_BLOB, PDF.as_bytes());
            http.blob(OTHER_BLOB, b"template");
            let path = match failed_stage {
                "tree" => "/repos/acme/tools/git/trees/HEAD?recursive=1".to_string(),
                "document" => format!("/repos/acme/tools/git/blobs/{PDF_BLOB}"),
                _ => format!("/repos/acme/tools/git/blobs/{OTHER_BLOB}"),
            };
            http.responses.insert(path, Err(failure.clone()));
            let http = AuthorizedHttp {
                http,
                token: lyrel_core::core::github::Token::new("synthetic-catalog-secret"),
            };
            let result = discover(&http, &GitHubSource::parse("acme/tools").unwrap())
                .and_then(|catalog| catalog.prepare(&http, 0));
            assert_eq!(result.unwrap_err(), CatalogError::Http(failure.clone()));
        }
    }
}

#[test]
fn github_authorization_is_not_accepted_by_skills_sh_search() {
    let http = AuthorizedHttp {
        http: Http::default(),
        token: lyrel_core::core::github::Token::new("synthetic-catalog-secret"),
    };
    let error = search(&http, "pdf").unwrap_err();
    assert!(matches!(error, lyrel_core::skills::catalog::CatalogError::Unsafe(_)));
    assert!(http.http.calls.lock().unwrap().is_empty());
}

#[test]
fn discovering_multiple_candidates_requires_explicit_prepare_and_preserves_ref_and_subpath() {
    let http = two_skills("release%2Fstable");
    let source = GitHubSource::parse("acme/tools/skills#release%2Fstable").unwrap();
    let catalog = discover(&http, &source).unwrap();
    assert_eq!(
        catalog
            .candidates()
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["other", "pdf"]
    );
    let prepared = catalog.prepare(&http, 1).unwrap();
    assert_eq!(
        prepared.candidate.source.reference.as_deref(),
        Some("release/stable")
    );
    assert_eq!(prepared.candidate.skill_path, "skills/pdf/SKILL.md");
    assert_eq!(prepared.candidate.tree_hash, PDF_TREE);
    assert_eq!(prepared.files.len(), 1);
    assert_eq!(prepared.files[0].path.to_str(), Some("SKILL.md"));
    assert_eq!(prepared.files[0].contents, PDF.as_bytes());
    assert!(
        http.calls
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.contains("archive"))
    );
    let only = discover(
        &http,
        &GitHubSource::parse("acme/tools/skills/pdf#release%2Fstable").unwrap(),
    )
    .unwrap();
    assert_eq!(only.candidates().len(), 1);
    assert_eq!(only.candidates()[0].name, "pdf");
}

#[test]
fn prepared_files_use_pinned_blobs_and_the_cli_locale_order_content_hash() {
    let mut http = Http::default();
    let lower = "6666666666666666666666666666666666666666";
    let upper = "7777777777777777777777777777777777777777";
    let last = "8888888888888888888888888888888888888888";
    http.tree(
        "HEAD",
        vec![
            file("SKILL.md", PDF_BLOB, PDF.len()),
            file("a.txt", lower, 5),
            file("B.txt", upper, 5),
            file("z.txt", last, 4),
        ],
    );
    http.blob(PDF_BLOB, PDF.as_bytes());
    http.blob(lower, b"lower");
    http.blob(upper, b"upper");
    http.blob(last, b"last");
    let catalog = discover(&http, &GitHubSource::parse("acme/tools").unwrap()).unwrap();
    let prepared = catalog.prepare(&http, 0).unwrap();
    assert_eq!(prepared.candidate.tree_hash, ROOT);
    assert_eq!(
        prepared.computed_hash,
        "07d70c66aeb165ff782ed41d0f557b5db7e69fa68134d33f6beec1353f80f709"
    );
    assert_eq!(
        http.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/git/trees/"))
            .count(),
        1,
        "prepare must not reread a moving branch"
    );
}

#[test]
fn unsafe_modes_paths_truncated_trees_and_oversized_documents_fail_closed() {
    for entry in [
        json!({"path":"link","mode":"120000","type":"blob","sha":PDF_BLOB,"size":1}),
        json!({"path":"module","mode":"160000","type":"commit","sha":ROOT}),
        file("../escape/SKILL.md", PDF_BLOB, 1),
        file("C:/escape/SKILL.md", PDF_BLOB, 1),
        file("bad\\escape/SKILL.md", PDF_BLOB, 1),
        file("SKILL.md", PDF_BLOB, 1024 * 1024 + 1),
    ] {
        let mut http = Http::default();
        http.tree("HEAD", vec![entry]);
        assert!(discover(&http, &GitHubSource::parse("acme/tools").unwrap()).is_err());
    }
    let mut http = two_skills("HEAD");
    http.json(
        "/repos/acme/tools/git/trees/HEAD?recursive=1",
        json!({"sha":ROOT,"truncated":true,"tree":[]}),
    );
    assert!(discover(&http, &GitHubSource::parse("acme/tools").unwrap()).is_err());
}

#[test]
fn malformed_skill_metadata_is_reported_without_fabricating_a_candidate() {
    let mut http = Http::default();
    let invalid = b"---\nname: pdf\n---\nMissing description";
    http.tree("HEAD", vec![file("SKILL.md", PDF_BLOB, invalid.len())]);
    http.blob(PDF_BLOB, invalid);
    let catalog = discover(&http, &GitHubSource::parse("acme/tools").unwrap()).unwrap();
    assert!(catalog.candidates().is_empty());
    assert!(catalog.problems()[0].contains("description"));
}

#[test]
fn search_distinguishes_empty_results_network_failure_and_real_source_selection() {
    let mut http = Http::default();
    http.json("/api/search?q=pdf%20tools&limit=20", json!({"skills":[{"id":"acme/tools/pdf","name":"PDF tools","source":"acme/tools","skillId":"pdf","installs":42,"future":true}],"future":{}}));
    let rows = search(&http, "pdf tools").unwrap();
    assert_eq!(rows[0].source.repository(), "acme/tools");
    assert_eq!(rows[0].source.skill_filter.as_deref(), Some("pdf"));
    assert_eq!(rows[0].installs, 42);
    assert!(
        http.calls
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.starts_with("/api/search?")),
        "search cannot install or discover without user selection"
    );
    http.json("/api/search?q=none&limit=20", json!({"skills":[]}));
    assert!(search(&http, "none").unwrap().is_empty());
    http.responses.insert(
        "/api/search?q=error&limit=20".into(),
        Err(ApiError::Http(503)),
    );
    assert!(search(&http, "error").is_err());
    http.json("/api/search?q=broken&limit=20", json!({"data":[]}));
    assert!(search(&http, "broken").is_err());
}

#[test]
fn preparation_rejects_file_count_total_bytes_and_blob_declaration_mismatches() {
    let source = GitHubSource::parse("acme/tools").unwrap();
    let mut http = Http::default();
    let mut entries = vec![file("SKILL.md", PDF_BLOB, PDF.len())];
    for index in 0..1000 {
        entries.push(file(&format!("file-{index}.txt"), OTHER_BLOB, 1));
    }
    http.tree("HEAD", entries);
    http.blob(PDF_BLOB, PDF.as_bytes());
    let catalog = discover(&http, &source).unwrap();
    assert!(
        catalog
            .prepare(&http, 0)
            .unwrap_err()
            .to_string()
            .contains("1,000")
    );
    http.tree(
        "HEAD",
        vec![
            file("SKILL.md", PDF_BLOB, PDF.len()),
            file("large.bin", OTHER_BLOB, 50 * 1024 * 1024),
        ],
    );
    let catalog = discover(&http, &source).unwrap();
    assert!(
        catalog
            .prepare(&http, 0)
            .unwrap_err()
            .to_string()
            .contains("50 MiB")
    );
    http.tree(
        "HEAD",
        vec![
            file("SKILL.md", PDF_BLOB, PDF.len()),
            file("data.bin", OTHER_BLOB, 4),
        ],
    );
    http.blob(OTHER_BLOB, b"wrong size");
    let catalog = discover(&http, &source).unwrap();
    assert!(catalog.prepare(&http, 0).is_err());
}

#[test]
fn duplicate_case_colliding_and_missing_parent_entries_are_rejected_before_fetching_blobs() {
    for entries in [
        vec![
            file("SKILL.md", PDF_BLOB, PDF.len()),
            file("skill.md", OTHER_BLOB, 1),
        ],
        vec![file("missing/SKILL.md", PDF_BLOB, PDF.len())],
        vec![
            file("file", OTHER_BLOB, 1),
            file("file/SKILL.md", PDF_BLOB, PDF.len()),
        ],
    ] {
        let mut http = Http::default();
        http.tree("HEAD", entries);
        assert!(discover(&http, &GitHubSource::parse("acme/tools").unwrap()).is_err());
        assert_eq!(http.calls.lock().unwrap().len(), 1);
    }
}

#[test]
fn yaml_frontmatter_is_really_parsed_and_skill_search_filters_the_same_catalog() {
    let mut http = Http::default();
    let yaml = "\u{feff}---\nname: 'PDF Tools'\ndescription: >-\n  Work with\n  PDF files\n---\n# Instructions\n";
    http.tree("HEAD", vec![file("SKILL.md", PDF_BLOB, yaml.len())]);
    http.blob(PDF_BLOB, yaml.as_bytes());
    let catalog = discover(&http, &GitHubSource::parse("acme/tools@pdf-tools").unwrap()).unwrap();
    assert_eq!(catalog.candidates()[0].description, "Work with PDF files");
    assert!(
        discover(&http, &GitHubSource::parse("acme/tools@missing").unwrap())
            .unwrap()
            .candidates()
            .is_empty()
    );
}

#[test]
#[ignore = "真实公开匿名 GitHub HTTP；仅在明确联网验收时执行，安装范围始终为 TempDir"]
fn public_github_install_is_immediately_browsable_with_cli_compatible_project_provenance() {
    use lyrel_core::{
        core::github::http::HttpTransport,
        host::local::LocalHost,
        skills::{
            ScanRequest,
            install::{
                InstallDecision, InstallStatus, InstallTarget, apply_install, preview_install,
            },
            scan,
        },
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let http = HttpTransport::new(None, None);
    let source =
        GitHubSource::parse("https://github.com/vercel-labs/skills/tree/main/skills/find-skills")
            .unwrap();
    let catalog = discover(&http, &source).unwrap();
    assert_eq!(catalog.candidates().len(), 1);
    assert_eq!(catalog.candidates()[0].name, "find-skills");
    let prepared = catalog.prepare(&http, 0).unwrap();
    let expected_hash = prepared.computed_hash.clone();
    let expected_markdown = prepared
        .files
        .iter()
        .find(|file| file.path == std::path::Path::new("SKILL.md"))
        .unwrap()
        .contents
        .clone();
    let host = LocalHost::new();
    let target = InstallTarget::project(project.clone()).unwrap();
    let preview = preview_install(&*host, target, vec![prepared]).unwrap();
    let report = apply_install(
        &*host,
        preview,
        &[InstallDecision::Install],
        "2026-10-19T08:00:00.000Z",
    );
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let launches = ["omp", "pi", "kimi", "opencode", "hermes"].into_iter().map(|agent| {
        let h = home.to_string_lossy();
        let config = if agent == "omp" { ".omp" } else { ".pi" };
        (agent.to_string(), format!("HOME='{h}' USERPROFILE='{h}' PROGRAMDATA='{h}/programdata' CLAUDE_CONFIG_DIR='{h}/.claude' XDG_DATA_HOME='{h}/.data' XDG_STATE_HOME='{h}/.state' PI_CONFIG_DIR={config} OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{h}/{config}/agent' KIMI_CODE_HOME='{h}/.kimi-code' HERMES_HOME='{h}/.hermes' XDG_CONFIG_HOME='{h}/.config' OPENCODE_CONFIG_DIR='{h}/.config/opencode' OPENCODE_CONFIG='{h}/opencode.json' {agent}"))
    }).collect();
    let request = ScanRequest::new(&home)
        .with_project(&project)
        .with_working_directory(&project)
        .with_launch_commands(launches);
    let snapshot = scan(&*host, &request).unwrap();
    let installed = snapshot
        .installations()
        .iter()
        .find(|s| s.name() == "find-skills")
        .expect("installed skill is immediately browsable");
    assert_eq!(installed.markdown().unwrap().as_bytes(), expected_markdown);
    let origin = installed
        .source_record()
        .expect("CLI lock source is visible in browsing");
    assert_eq!(origin.source(), "vercel-labs/skills");
    assert_eq!(origin.reference(), Some("main"));
    assert_eq!(origin.skill_path(), Some("skills/find-skills/SKILL.md"));
    let lock: Value =
        serde_json::from_slice(&std::fs::read(project.join("skills-lock.json")).unwrap()).unwrap();
    assert_eq!(lock["version"], 1);
    assert_eq!(lock["skills"]["find-skills"]["computedHash"], expected_hash);
    assert!(
        !home.join(".agents").exists(),
        "project installation must not write a user shared directory"
    );
    eprintln!(
        "public install verified: tree={} project={} skill=skills/find-skills/SKILL.md",
        catalog.tree_sha(),
        project.display()
    );
}

#[test]
fn explicit_ref_and_subpath_overrides_do_not_reinterpret_skill_delimiters() {
    let source = GitHubSource::parse("acme/tools/skills#original@pdf").unwrap();
    let selected = source
        .clone()
        .with_selection(Some("feature/@pdf#review"), Some("skills/pdf"))
        .unwrap();
    assert_eq!(selected.reference.as_deref(), Some("feature/@pdf#review"));
    assert_eq!(selected.skill_filter.as_deref(), Some("pdf"));
    assert_eq!(selected.subpath.as_deref(), Some("skills/pdf"));
    let http = two_skills("feature%2F%40pdf%23review");
    assert_eq!(
        discover(&http, &selected).unwrap().candidates()[0].name,
        "pdf"
    );
    assert_eq!(source.clone().with_selection(None, None).unwrap(), source);
    assert!(
        source
            .clone()
            .with_selection(Some("bad ref"), None)
            .is_err()
    );
    assert!(source.with_selection(None, Some("../escape")).is_err());
}

#[test]
fn selecting_multiple_skills_checks_one_download_budget_before_companion_requests() {
    let mut http = two_skills("HEAD");
    let mut entries = vec![
        dir("skills", ROOT),
        dir("skills/pdf", PDF_TREE),
        file("skills/pdf/SKILL.md", PDF_BLOB, PDF.len()),
        dir("skills/other", OTHER_TREE),
        file("skills/other/SKILL.md", OTHER_BLOB, OTHER.len()),
    ];
    for folder in ["pdf", "other"] {
        for file_ix in 0..600 {
            entries.push(file(&format!("skills/{folder}/file-{file_ix}"), ROOT, 1));
        }
    }
    http.tree("HEAD", entries);
    let catalog = discover(&http, &GitHubSource::parse("acme/tools").unwrap()).unwrap();
    assert!(
        catalog
            .prepare_many(&http, &[0, 1])
            .unwrap_err()
            .to_string()
            .contains("1,000")
    );
}

#[test]
#[ignore = "真实公开 skills.sh HTTP；仅在明确联网验收时执行，不写任何安装目录"]
fn public_skills_sh_search_exposes_a_reviewable_github_source() {
    let http = lyrel_core::skills::catalog::SkillsSearchHttp::new(None);
    let results = search(&http, "find-skills").unwrap();
    let result = results
        .iter()
        .find(|result| {
            result.source.repository() == "vercel-labs/skills" && result.skill_id == "find-skills"
        })
        .expect("published find-skills source should be reviewable");
    assert_eq!(result.source.skill_filter.as_deref(), Some("find-skills"));
    eprintln!(
        "public search verified: {} @ {}",
        result.source.repository(),
        result.skill_id
    );
}

#[test]
fn supported_github_search_results_survive_mixed_non_github_catalogue_entries() {
    let mut http = Http::default();
    http.json("/api/search?q=find%2Dskills&limit=20", json!({"skills":[
        {"id":"vercel-labs/skills/find-skills","source":"vercel-labs/skills","skillId":"find-skills","name":"find-skills","installs":3917059},
        {"id":"skills.volces.com/find-skills","source":"skills.volces.com","skillId":"find-skills","name":"find-skills","installs":1561},
        {"id":"modelscope.cn/find-skills","source":"modelscope.cn","skillId":"find-skills","name":"find-skills","installs":1375}
    ]}));
    let results = search(&http, "find-skills").unwrap();
    assert_eq!(
        results
            .iter()
            .map(|result| result.source.repository())
            .collect::<Vec<_>>(),
        ["vercel-labs/skills"]
    );
    assert_eq!(
        results[0].source.skill_filter.as_deref(),
        Some("find-skills")
    );
}
