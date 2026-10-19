use super::*;
use crate::host::local::LocalHost;
use serde_json::json;

fn session(root: &std::path::Path, id: &str, parent: Option<&str>, child: bool) -> PathBuf {
    let dir = root.join("wd_work_0123456789ab").join(id);
    std::fs::create_dir_all(dir.join("session-meta")).unwrap();
    std::fs::write(dir.join("session-meta/state.json"), json!({
        "id":id,"cwd":"/work","createdAt":1792320000000i64,"forkedFrom":parent,
        "custom":if child {json!({"parent_session_id":parent,"child_session_kind":"child"})} else {json!({})}
    }).to_string()).unwrap();
    dir
}
fn wire(dir: &std::path::Path, agent: &str, body: &str) {
    let path = dir.join("agents").join(agent);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("wire.jsonl"), format!("{{\"type\":\"metadata\",\"protocol_version\":\"1.5\",\"created_at\":1792320000000}}\n{body}")).unwrap();
}
fn usage(agent: &str, scope: &str, input: u64) -> String {
    format!("{}\n",json!({"type":"usage.record","agentId":agent,"model":"kimi-code/kimi-for-coding","usageScope":scope,"time":1792320001000i64,"usage":{"inputOther":input,"output":2,"inputCacheRead":3,"inputCacheCreation":0},"private":"PRIVATE BODY"}))
}
#[test]
fn statistics_kimi_all_scopes_subagents_and_fork_boundaries_are_native_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let main = session(&root,"session_main",None,false);
    wire(&main,"main",&(usage("main","turn",10)+&usage("main","session",20)));
    wire(&main,"agent-1",&usage("agent-1","turn",30));
    let fork = session(&root,"session_fork",Some("session_main"),false);
    wire(&fork,"main",&(usage("main","turn",10)+"{\"type\":\"forked\",\"agentId\":\"main\",\"time\":1792320002000}\n"+&usage("main","turn",40)));
    wire(&fork,"agent-1",&(usage("agent-1","turn",30)+"{\"type\":\"forked\",\"agentId\":\"agent-1\",\"time\":1792320002000}\n"+&usage("agent-1","session",50)));
    let child = session(&root,"session_child",Some("session_main"),true);
    wire(&child,"main",&(usage("main","turn",10)+"{\"type\":\"forked\",\"agentId\":\"main\",\"time\":1792320002000}\n"+&usage("main","turn",60)));
    let snapshot = scan(&*LocalHost::new(), &[Source::kimi(root,"kimi-home")], &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert!(snapshot.is_complete(),"{:?}",snapshot.issues());
    assert_eq!(totals.calls(),6);
    assert_eq!(totals.tokens().total(),Some(240));
    assert_eq!(totals.tokens().input(),Some(210));
    assert_eq!(totals.main_sessions(),2);
    assert_eq!(totals.recorded_usd(),None);
    assert_eq!(totals.unpriced_calls(),6);
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("PRIVATE BODY"));
}

#[test]
fn statistics_kimi_missing_fork_boundary_is_visible_without_hiding_other_agents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let main = session(&root,"session_main",None,false);
    wire(&main,"main",&usage("main","session",10));
    let fork = session(&root,"session_fork",Some("session_missing"),false);
    wire(&fork,"main",&usage("main","turn",1000));
    let snapshot = scan(&*LocalHost::new(),&[Source::kimi(root,"same")],&mut StatisticsCache::default()).unwrap();
    assert!(!snapshot.is_complete());
    assert!(snapshot.issues().iter().any(|issue|issue.reason().contains("forked marker")));
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(),Some(15));
    assert_eq!(totals.main_sessions(),1);
}

#[test]
fn statistics_kimi_persisted_calls_have_no_turn_dedup_and_missing_categories_stay_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let main = session(temp.path(),"session_main",None,false);
    // Published native sample: ccusage 69fbca0, independently checked against first-party schema.
    let sample = "{\"type\":\"usage.record\",\"agentId\":\"main\",\"model\":\"kimi-code/kimi-for-coding\",\"usage\":{\"inputOther\":3064,\"output\":76,\"inputCacheRead\":14848,\"inputCacheCreation\":0},\"usageScope\":\"turn\",\"time\":1782113184943}\n";
    wire(&main,"main",&(sample.to_owned()+sample+"{\"type\":\"usage.record\",\"agentId\":\"main\",\"model\":\"other\",\"usage\":{\"inputOther\":0,\"output\":0,\"inputCacheRead\":0},\"time\":1782113184944}\n"));
    let snapshot = scan(&*LocalHost::new(),&[Source::kimi(temp.path().into(),"a")],&mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.calls(),3);
    assert_eq!(totals.tokens().input(),Some(6128));
    assert_eq!(totals.tokens().output(),Some(152));
    assert_eq!(totals.tokens().cache_read(),Some(29696));
    assert_eq!(totals.tokens().cache_write(),None);
    assert_eq!(totals.tokens().total(),None);
    assert_eq!(totals.unpriced_calls(),3);
}

#[test]
fn statistics_kimi_empty_forks_are_not_active_and_new_subagents_are_counted() {
    let temp = tempfile::tempdir().unwrap();
    let fork = session(temp.path(),"session_fork",Some("session_missing"),false);
    wire(&fork,"main",&(usage("main","turn",1000)+"{\"type\":\"forked\",\"agentId\":\"main\",\"time\":1792320002000}\n"));
    wire(&fork,"agent-2",&usage("agent-2","turn",10));
    let path = fork.join("agents/agent-2/wire.jsonl");
    let data = std::fs::read_to_string(&path).unwrap().replace("\"created_at\":1792320000000","\"created_at\":1792320000001");
    std::fs::write(path,data).unwrap();
    let snapshot = scan(&*LocalHost::new(),&[Source::kimi(temp.path().into(),"a")],&mut StatisticsCache::default()).unwrap();
    assert!(snapshot.is_complete(),"{:?}",snapshot.issues());
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(),0);
    assert_eq!(totals.tokens().total(),Some(15));
}

#[test]
fn statistics_kimi_scope_discovery_rejects_ambiguous_home_and_respects_custom_home() {
    let host = LocalHost::new();
    for selector in ["", "relative"] {
        let launch = HashMap::from([("kimi".into(),format!("KIMI_CODE_HOME='{selector}' kimi"))]);
        assert!(discover_kimi(&*host,&launch).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let launch = HashMap::from([("kimi".into(),format!("KIMI_CODE_HOME='{}' kimi",temp.path().display()))]);
    let sources = discover_kimi(&*host,&launch).unwrap();
    assert_eq!(sources.len(),1);
    assert!(sources[0].root().ends_with("sessions"));
    let session = session(sources[0].root(),"session_main",None,false);
    wire(&session,"main",&usage("main","session",10));
    let snapshot = scan(&*host,&sources,&mut StatisticsCache::default()).unwrap();
    assert_eq!(snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),Some(15));
}

#[test]
fn statistics_kimi_cache_does_not_resurrect_archived_or_deleted_sessions_on_partial_failure() {
    let temp = tempfile::tempdir().unwrap();
    let archived = session(temp.path(),"session_archived",None,false);
    let deleted = session(temp.path(),"session_deleted",None,false);
    let retained = session(temp.path(),"session_retained",None,false);
    for dir in [&archived,&deleted,&retained] { wire(dir,"main",&usage("main","turn",10)); }
    let host = LocalHost::new();
    let sources = [Source::kimi(temp.path().into(),"a")];
    let mut cache = StatisticsCache::default();
    let before = scan(&*host,&sources,&mut cache).unwrap();
    assert_eq!(before.aggregate(&DateWindow::all("UTC")).calls(),3);
    let meta = archived.join("session-meta/state.json");
    let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    state["archived"] = json!(true);
    std::fs::write(meta,state.to_string()).unwrap();
    std::fs::remove_dir_all(deleted).unwrap();
    std::fs::write(retained.join("agents/main/wire.jsonl"),"broken").unwrap();
    let after = scan(&*host,&sources,&mut cache).unwrap();
    assert!(!after.is_complete());
    assert_eq!(after.aggregate(&DateWindow::all("UTC")).calls(),1);
}

#[test]
#[ignore = "requires an explicit native Kimi home; prints metering only"]
fn statistics_kimi_native_home_smoke() {
    let root = PathBuf::from(std::env::var("LYREL_TEST_KIMI_HOME").unwrap());
    let host = LocalHost::new();
    let launch = HashMap::from([("kimi".into(),format!("KIMI_CODE_HOME='{}' kimi",root.display()))]);
    let sources = discover_kimi(&*host,&launch).unwrap();
    let mut cache = StatisticsCache::default();
    for _ in 0..2 {
        let snapshot = scan(&*host,&sources,&mut cache).unwrap();
        let totals = snapshot.aggregate(&DateWindow::all("UTC"));
        println!("Kimi native metering: sources={} calls={} main_sessions={} tokens={:?} unpriced={} issues={}",sources.len(),totals.calls(),totals.main_sessions(),totals.tokens().total(),totals.unpriced_calls(),snapshot.issues().len());
        assert!(totals.calls()>0,"native smoke requires real durable usage");
        assert_eq!(totals.recorded_usd(),None);
    }
}
