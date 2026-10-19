use std::{collections::HashMap, fs, path::Path};
use lyrel_core::{host::local::LocalHost, skills::{ScanRequest, SourceKind, scan}};

fn fixture(home: &Path, project: &Path) -> ScanRequest {
    ScanRequest::new(home).with_project(project).with_launch_commands(fixture_launches(home))
}
fn skill(root: &Path, name: &str, description: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n# Actual instructions\nRead the companion file.\n")).unwrap();
}

#[test]
fn browsing_keeps_same_names_at_distinct_installations_and_reads_actual_details() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let shared = home.join(".agents/skills/review");
    let private = home.join(".pi/agent/skills/review");
    skill(&shared, "review", "Shared review");
    skill(&private, "review", "Private review");
    skill(&project.join(".agents/skills/review"), "review", "Project review");
    fs::write(shared.join("guide.txt"), "reference").unwrap();
    fs::write(home.join("unrelated.txt"), "keep exactly").unwrap();
    let host = LocalHost::new();
    let snapshot = scan(&*host, &fixture(&home, &project)).unwrap();
    let reviews = snapshot.filtered("review", None).collect::<Vec<_>>();
    assert_eq!(reviews.len(), 3, "same name is not installation identity");
    let shared = reviews.iter().find(|s| s.path() == host.canonicalize(&shared).unwrap()).unwrap();
    assert_eq!(shared.description(), Some("Shared review"));
    assert!(shared.markdown().unwrap().contains("# Actual instructions"));
    assert_eq!(shared.companions().files(), 1);
    assert_eq!(shared.companions().bytes(), 9);
    assert_eq!(snapshot.filtered("Private review", Some(SourceKind::AgentPrivate)).count(), 1);
    assert_eq!(fs::read_to_string(home.join("unrelated.txt")).unwrap(), "keep exactly");
    assert_eq!(fs::read_to_string(private.join("SKILL.md")).unwrap(), "---\nname: review\ndescription: Private review\n---\n# Actual instructions\nRead the companion file.\n");
}

#[test]
fn configured_profiles_and_native_discovery_depth_remain_distinct() {
    let temp=tempfile::tempdir().unwrap();
    let home=temp.path().join("home");let project=temp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    skill(&home.join(".omp/profiles/research/agent/skills/omp-only"),"profile-skill","Selected omp profile");
    skill(&home.join(".omp/agent/skills/default-only"),"default-only","Not selected");
    skill(&home.join(".pi/agent/skills/group/pi-only"),"pi-only","Recursive pi");
    skill(&home.join(".omp/profiles/research/agent/skills/group/hidden"),"omp-nested","Not a native omp skill root");
    let mut request=fixture(&home,&project);
    let launch=request_launch(&home,"omp");
    let launches=HashMap::from([("omp".into(),format!("{launch} --profile research"))]);
    // Preserve all other agents' isolated launch roots.
    let mut all=fixture_launches(&home);all.extend(launches);
    request=request.with_launch_commands(all);
    let host=LocalHost::new();let result=scan(&*host,&request).unwrap();
    assert_eq!(result.filtered("Selected omp profile",None).count(),1);
    assert_eq!(result.filtered("default-only",None).count(),0);
    assert_eq!(result.filtered("Recursive pi",None).count(),1);
    assert_eq!(result.filtered("omp-nested",None).count(),0);
}

fn fixture_launches(home:&Path)->HashMap<String,String> {
    ["omp","pi","kimi","opencode","hermes"].into_iter().map(|agent|(agent.into(),request_launch(home,agent))).collect()
}
fn request_launch(home:&Path,agent:&str)->String {
    let home=home.to_string_lossy();let agent_dir=if agent=="omp"{format!("{home}/.omp/agent")}else{format!("{home}/.pi/agent")};
    format!("HOME='{home}' USERPROFILE='{home}' PROGRAMDATA='{home}/programdata' CLAUDE_CONFIG_DIR='{home}/.claude' XDG_DATA_HOME='{home}/.data' XDG_STATE_HOME='{home}/.state' PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{agent_dir}' KIMI_CODE_HOME='{home}/.kimi-code' HERMES_HOME='{home}/.hermes' XDG_CONFIG_HOME='{home}/.config' OPENCODE_CONFIG_DIR='{home}/.config/opencode' {agent}")
}

#[test]
fn unreadable_size_and_invalid_frontmatter_are_not_success_or_fake_metadata() {
    let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let project=temp.path().join("project");fs::create_dir_all(&project).unwrap();
    let invalid=home.join(".agents/skills/invalid");fs::create_dir_all(&invalid).unwrap();fs::write(invalid.join("SKILL.md"),"---\nname: [broken\n---\nReal body").unwrap();
    let huge=home.join(".agents/skills/huge");fs::create_dir_all(&huge).unwrap();fs::write(huge.join("SKILL.md"),vec![b'x';1024*1024+1]).unwrap();
    let host=LocalHost::new();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    let bad=result.installations().iter().find(|s|s.path()==host.canonicalize(&invalid).unwrap()).unwrap();
    assert!(bad.description().is_none());assert!(bad.origin().is_none());assert!(bad.markdown().unwrap().contains("Real body"));assert!(!bad.problems().is_empty());
    let huge=result.installations().iter().find(|s|s.path()==host.canonicalize(&huge).unwrap()).unwrap();
    assert!(huge.markdown().is_none());assert!(huge.problems().iter().any(|p|p.contains("1048576")));
}

#[test]
fn native_source_opt_out_and_flat_kimi_files_are_visible_without_enabling_them() {
    use lyrel_core::{core::agent_hooks::HookAgent,skills::DiscoveryState};
    let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let project=temp.path().join("project");fs::create_dir_all(&project).unwrap();
    skill(&home.join(".agents/skills/shared"),"shared","Cross-tool skill");
    fs::create_dir_all(home.join(".kimi-code/skills")).unwrap();fs::write(home.join(".kimi-code/skills/flat.md"),"Flat description\n\nActual flat instructions").unwrap();
    let host=LocalHost::new();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    let shared=result.filtered("Cross-tool",None).next().unwrap();
    assert!(shared.sources().iter().any(|s|s.agent()==HookAgent::Hermes&&s.state()==DiscoveryState::Disabled));
    let flat=result.filtered("Flat description",None).next().unwrap();assert_eq!(flat.name(),"flat");assert_eq!(flat.document(),host.canonicalize(&home.join(".kimi-code/skills/flat.md")).unwrap());
    assert!(!home.join(".hermes/config.yaml").exists());
}

#[test]
fn link_aliases_share_installation_identity_without_merging_real_copies() {
    let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let project=temp.path().join("project");fs::create_dir_all(&project).unwrap();
    let original=home.join(".agents/skills/original");let alias=home.join(".agents/skills/alias");let copy=home.join(".agents/skills/copy");
    skill(&original,"same-name","Real target");skill(&copy,"same-name","Different content");
    #[cfg(unix)] std::os::unix::fs::symlink("original",&alias).unwrap();
    #[cfg(windows)] {
        let output=std::process::Command::new("cmd").args(["/D","/C","mklink","/J"]).arg(&alias).arg(&original).output().unwrap();
        assert!(output.status.success(),"junction prerequisite: {}",String::from_utf8_lossy(&output.stderr));
    }
    let host=LocalHost::new();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    assert_eq!(result.filtered("same-name",None).count(),2);
    let target=result.filtered("Real target",None).next().unwrap();
    assert!(target.sources().iter().any(|s|s.entry()==alias));
    assert!(target.sources().iter().any(|s|s.entry()==original));
    assert_eq!(fs::read_to_string(copy.join("SKILL.md")).unwrap(),"---\nname: same-name\ndescription: Different content\n---\n# Actual instructions\nRead the companion file.\n");
}

#[test]
fn configured_extra_directories_jsonc_and_installed_package_resources_are_browsable() {
    let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let project=temp.path().join("project");fs::create_dir_all(&project).unwrap();
    let extra=project.join("extra-skills");skill(&extra.join("custom"),"custom","Custom configured source");
    fs::create_dir_all(home.join(".config/opencode")).unwrap();
    fs::write(home.join(".config/opencode/opencode.jsonc"),"{\n// a native JSONC comment\n\"skills\": [\"extra-skills\",],\n}").unwrap();
    let package=home.join(".pi/agent/npm/node_modules/reference-pack");
    skill(&package.join("knowledge/reference"),"package-reference","Installed package reference");
    fs::write(package.join("package.json"),r#"{"name":"reference-pack","pi":{"skills":["knowledge/*"]}}"#).unwrap();
    let host=LocalHost::new();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    assert_eq!(result.filtered("Custom configured source",None).count(),1);
    assert_eq!(result.filtered("Installed package reference",None).count(),1);
    assert!(!result.problems().iter().any(|p|p.contains("opencode.jsonc")),"valid JSONC must remain a usable source");
    assert_eq!(fs::read_to_string(package.join("package.json")).unwrap(),r#"{"name":"reference-pack","pi":{"skills":["knowledge/*"]}}"#);
}

#[test]
fn source_records_bind_to_shared_real_paths_not_same_named_private_copies() {
    let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let project=temp.path().join("project");fs::create_dir_all(&project).unwrap();
    skill(&home.join(".agents/skills/pdf"),"pdf","Shared PDF");skill(&home.join(".pi/agent/skills/pdf"),"pdf","Private PDF");
    fs::create_dir_all(home.join(".state/skills")).unwrap();
    let lock=home.join(".state/skills/.skill-lock.json");
    let recorded=r#"{"version":3,"future":{"keep":true},"skills":{"pdf":{"source":"anthropics/skills","sourceType":"github","sourceUrl":"https://github.com/anthropics/skills.git","skillPath":"skills/pdf/SKILL.md","ref":"main","skillFolderHash":"1111111111111111111111111111111111111111","installedAt":"2026-10-01T00:00:00Z","updatedAt":"2026-10-01T00:00:00Z"}}}"#;
    fs::write(&lock,recorded).unwrap();
    let host=LocalHost::new();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    let shared=result.filtered("Shared PDF",None).next().unwrap().source_record().unwrap();
    assert_eq!(shared.source(),"anthropics/skills");assert_eq!(shared.reference(),Some("main"));assert_eq!(shared.skill_path(),Some("skills/pdf/SKILL.md"));
    assert!(result.filtered("Private PDF",None).next().unwrap().source_record().is_none());
    assert_eq!(fs::read_to_string(&lock).unwrap(),recorded);
    fs::write(&lock,"{broken lock").unwrap();let result=scan(&*host,&fixture(&home,&project)).unwrap();
    assert!(result.filtered("Shared PDF",None).next().unwrap().source_record().is_none());
    assert!(result.problems().iter().any(|p|p.contains("source record")));
    assert_eq!(fs::read_to_string(&lock).unwrap(),"{broken lock");
}
