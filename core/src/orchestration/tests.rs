use super::*;
use std::path::Path;

fn fixture() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "comrade-native-test-{}-{}",
        std::process::id(),
        IDS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn spec(id: &str, deps: &[&str]) -> TaskSpec {
    TaskSpec {
        id: id.into(),
        task: id.into(),
        agent: "codex".into(),
        model: String::new(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
    }
}
fn plan(dir: &Path, tasks: Vec<TaskSpec>) -> Plan {
    Plan {
        title: "Build feature".into(),
        working_dir: dir.to_string_lossy().into(),
        tasks,
        max_parallel: 2,
        timeout_secs: 600,
    }
}
#[test]
fn rejects_bad_graphs_and_limits() {
    let dir = Path::new("/project");
    assert!(validate(&plan(dir, vec![spec("a", &[]), spec("b", &["a"])])).is_ok());
    for tasks in [
        vec![],
        vec![spec("a", &["missing"])],
        vec![spec("a", &["b"]), spec("b", &["a"])],
        vec![spec("a", &[]), spec("A", &[])],
        vec![spec("../a", &[])],
    ] {
        assert!(validate(&plan(dir, tasks)).is_err());
    }
    let mut p = plan(dir, vec![spec("a", &[])]);
    p.max_parallel = 4;
    assert!(validate(&p).is_err());
    p.max_parallel = 1;
    p.timeout_secs = 0;
    assert!(validate(&p).is_err());
    let mut s = spec("a", &[]);
    s.agent = "unknown".into();
    assert!(validate(&plan(dir, vec![s])).is_err());
    let mut s = spec("a", &[]);
    s.model = "--yolo".into();
    assert!(validate(&plan(dir, vec![s])).is_err());
}
#[test]
fn prompts_stay_single_arguments_and_permissions_are_not_bypassed() {
    let prompt = "-x $(touch /host) `danger`\nhello";
    for a in adapters::adapters() {
        let args = adapters::argv(a.id, prompt, "model/name").unwrap();
        assert_eq!(args.iter().filter(|s| s.as_str() == prompt).count(), 1);
        assert!(args.contains(&"model/name".into()));
        assert!(!args.iter().any(|a| a.contains("bypass")
            || a == "yolo"
            || a.starts_with("--allow-tool")
            || a == "--allowedTools"));
    }
}
#[cfg(unix)]
struct Environment {
    dir: PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
#[cfg(unix)]
impl Environment {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture();
        let previous = ["PATH", "COMRADE_HOME", "OPENAI_API_KEY"]
            .into_iter()
            .map(|k| (k, std::env::var_os(k)))
            .collect();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = r#"#!/bin/sh
for arg in "$@"; do
case "$arg" in
  'Task: first'*) printf 'first edit\n' > first.txt; printf 'first running\n'; /bin/sleep .15; printf 'key=%s\n' "$OPENAI_API_KEY"; exit 0;;
  'Task: second'*) test -f first.txt || exit 9; /bin/cat first.txt > second.txt; printf 'dependency visible\n'; exit 0;;
  'Task: parallel-a'*|'Task: parallel-b'*) printf 'started\n'; /bin/sleep .4; exit 0;;
  'Task: failure'*) printf 'fixture failed\n' >&2; exit 7;;
  'Task: slow'*) (/bin/sleep 1; printf 'escaped\n' > escaped.txt) & printf 'slow started working........................................\n'; /bin/sleep 30; exit 0;;
esac
done
exit 8
"#;
        for name in ["codex", "opencode"] {
            let path = bin.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PATH", &bin);
        std::env::set_var("COMRADE_HOME", &dir);
        std::env::set_var("OPENAI_API_KEY", "fixture-secret-value");
        Self { dir, previous }
    }
    fn manager(&self) -> Arc<Orchestrator> {
        Arc::new(Orchestrator::new(self.dir.join("state")))
    }
}
#[cfg(unix)]
impl Drop for Environment {
    fn drop(&mut self) {
        for (k, v) in &self.previous {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn wait_done(manager: &Orchestrator, id: &str) -> Run {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let r = manager.get(id).unwrap();
            if r.status.terminal() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("run finished")
}
#[cfg(unix)]
#[tokio::test]
async fn installed_agents_edit_shared_project_and_obey_dependencies() {
    let _lock = crate::env_lock();
    let env = Environment::new();
    let manager = env.manager();
    let runtime = manager.runtime().await;
    assert!(runtime.ready);
    assert!(runtime
        .agents
        .iter()
        .any(|a| a.id == "opencode" && a.available));
    assert!(!runtime
        .agents
        .iter()
        .any(|a| a.id == "hermes" && a.available));
    let mut second = spec("second", &["first"]);
    second.agent = "opencode".into();
    let run = manager
        .start(plan(&env.dir, vec![spec("first", &[]), second]))
        .await
        .unwrap();
    let done = wait_done(&manager, &run.id).await;
    assert_eq!(done.status, Status::Succeeded);
    assert_eq!(
        std::fs::read_to_string(env.dir.join("second.txt")).unwrap(),
        "first edit\n"
    );
    assert!(done.jobs[0].log.contains("[REDACTED]"));
    assert!(!done.jobs[0].log.contains("fixture-secret-value"));
    assert!(done.jobs[1].started_at.unwrap() >= done.jobs[0].finished_at.unwrap());
    let restored = Orchestrator::new(env.dir.join("state"));
    assert_eq!(restored.get(&run.id).unwrap().jobs[1].log, done.jobs[1].log);
}
#[cfg(unix)]
#[tokio::test]
async fn independent_agents_run_concurrently_and_failed_dependencies_block() {
    let _lock = crate::env_lock();
    let env = Environment::new();
    let manager = env.manager();
    let run = manager
        .start(plan(
            &env.dir,
            vec![spec("parallel-a", &[]), spec("parallel-b", &[])],
        ))
        .await
        .unwrap();
    let done = wait_done(&manager, &run.id).await;
    assert_eq!(done.status, Status::Succeeded);
    assert!(done.jobs[1].started_at.unwrap() < done.jobs[0].finished_at.unwrap());
    let run = manager
        .start(plan(
            &env.dir,
            vec![spec("failure", &[]), spec("second", &["failure"])],
        ))
        .await
        .unwrap();
    let done = wait_done(&manager, &run.id).await;
    assert_eq!(done.jobs[0].status, Status::Failed);
    assert_eq!(done.jobs[1].status, Status::Blocked);
    assert!(!env.dir.join("second.txt").exists());
}
#[cfg(unix)]
#[tokio::test]
async fn cancellation_stops_descendants_and_blocks_downstream_work() {
    let _lock = crate::env_lock();
    let env = Environment::new();
    let manager = env.manager();
    let run = manager
        .start(plan(
            &env.dir,
            vec![spec("slow", &[]), spec("second", &["slow"])],
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if manager.get(&run.id).unwrap().jobs[0]
                .log
                .contains("slow started")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel(&run.id, Some("slow")).await.unwrap();
    let done = wait_done(&manager, &run.id).await;
    assert_eq!(done.jobs[0].status, Status::Cancelled);
    assert_eq!(done.jobs[1].status, Status::Blocked);
    assert!(manager.processes.lock().unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(1150)).await;
    assert!(!env.dir.join("escaped.txt").exists());
    assert_eq!(
        manager.get(&run.id).unwrap().jobs[0].status,
        Status::Cancelled
    );
}
#[cfg(unix)]
#[tokio::test]
async fn missing_agents_invalid_paths_and_disabled_agents_do_not_launch() {
    let _lock = crate::env_lock();
    let env = Environment::new();
    let manager = env.manager();
    let mut task = spec("a", &[]);
    task.agent = "hermes".into();
    assert!(manager.start(plan(&env.dir, vec![task])).await.is_err());
    assert!(manager
        .start(plan(Path::new("relative"), vec![spec("a", &[])]))
        .await
        .is_err());
    let mut prefs = crate::prefs::load();
    prefs.coding.agents = vec!["opencode".into()];
    crate::prefs::save(&prefs).unwrap();
    assert!(manager
        .start(plan(&env.dir, vec![spec("a", &[])]))
        .await
        .is_err());
    assert!(manager.list().is_empty());
}
#[tokio::test]
async fn streamed_redaction_handles_split_keys_unicode_and_log_limits() {
    let dir = fixture();
    let manager = Orchestrator::new(dir.clone());
    let p = plan(Path::new("/project"), vec![spec("a", &[])]);
    let run = Run {
        id: "run".into(),
        plan: p.clone(),
        status: Status::Running,
        created_at: now(),
        jobs: vec![Job {
            spec: p.tasks[0].clone(),
            status: Status::Running,
            started_at: Some(now()),
            finished_at: None,
            log: String::new(),
            error: None,
        }],
    };
    manager.runs.lock().unwrap().insert(run.id.clone(), run);
    let (mut tx, rx) = tokio::io::duplex(32);
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        for part in ["unicode 界 sec", "ret-key tail\n"] {
            tx.write_all(part.as_bytes()).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    manager
        .read_log("run", "a", rx, &["secret-key".into()])
        .await
        .unwrap();
    writer.await.unwrap();
    let log = manager.get("run").unwrap().jobs[0].log.clone();
    assert!(log.contains("界 [REDACTED] tail"), "{log}");
    manager.log("run", "a", &"界".repeat(MAX_LOG));
    assert!(manager.get("run").unwrap().jobs[0].log.len() <= MAX_LOG);
    manager.persist(&manager.runs.lock().unwrap()).unwrap();
    let restored = Orchestrator::new(dir.clone());
    assert_eq!(
        restored.get("run").unwrap().jobs[0].status,
        Status::Interrupted
    );
    assert!(restored.processes.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_terminates_the_worker_and_output_capture_is_bounded() {
    let _lock = crate::env_lock();
    let env = Environment::new();
    let manager = env.manager();
    // Exercise a short deadline directly; public plans enforce a 30s minimum.
    let mut p = plan(&env.dir, vec![spec("slow", &[])]);
    p.timeout_secs = 1;
    let task = p.tasks[0].clone();
    manager.runs.lock().unwrap().insert(
        "deadline".into(),
        Run {
            id: "deadline".into(),
            plan: p,
            created_at: now(),
            status: Status::Running,
            jobs: vec![Job {
                spec: task.clone(),
                status: Status::Running,
                started_at: Some(now()),
                finished_at: None,
                log: String::new(),
                error: None,
            }],
        },
    );
    let result = manager.execute("deadline", &task).await;
    assert!(result.unwrap_err().contains("Deadline exceeded"));
    assert!(manager.processes.lock().unwrap().is_empty());
    let error = manager
        .read_log("deadline", "slow", tokio::io::repeat(b'x'), &[])
        .await
        .unwrap_err();
    assert!(error.contains("output budget exceeded"));
    assert!(manager.get("deadline").unwrap().jobs[0].log.len() <= MAX_LOG);
}

#[test]
fn batch_task_encoding_preserves_multiline_text_in_one_regular_argument() {
    let original = "first line\r\nsecond line\nUnicode café 🧪 \"quotes\" %PATH% !variable! & | < > ^ $(touch /host) `command`";
    let encoded = adapters::task_argument(original, true).unwrap();
    assert!(!encoded.contains('\r'));
    assert!(!encoded.contains('\n'));
    let json = encoded
        .strip_prefix("Decode this JSON string as the task text, preserving escaped newlines: ")
        .unwrap();
    assert_eq!(serde_json::from_str::<String>(json).unwrap(), original);
    assert_eq!(adapters::task_argument(original, false).unwrap(), original);
    for adapter in adapters::adapters() {
        let args = adapters::argv(adapter.id, &encoded, "model/name").unwrap();
        assert_eq!(args.iter().filter(|arg| arg.as_str() == encoded).count(), 1);
        assert!(!args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"));
    }
}

#[test]
fn native_executable_task_arguments_remain_unchanged() {
    let original = "first\nsecond\r\n%PATH% & text";
    for binary in [Path::new("agent.exe"), Path::new("agent")] {
        let args = adapters::argv_for_binary("opencode", original, "", binary).unwrap();
        assert_eq!(args.last().unwrap(), original);
    }
    #[cfg(not(windows))]
    assert_eq!(
        adapters::argv_for_binary("opencode", original, "", Path::new("agent.cmd"))
            .unwrap()
            .last()
            .unwrap(),
        original
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_batch_node_fixture_receives_encoded_task_without_shell_expansion() {
    let _node = crate::prefs::find_on_path("node")
        .expect("Node is required for the Windows batch fixture (provided by CI setup-node)");
    let dir = fixture();
    let result = async {
        let script = dir.join("receive argv.js");
        std::fs::write(&script, "process.stdout.write(JSON.stringify(process.argv.slice(2)));").unwrap();
        let batch = dir.join("fixture agent.CmD");
        std::fs::write(&batch, format!("@echo off\r\nnode.exe \"{}\" %*\r\n", script.display())).unwrap();
        let work = dir.join("other project");
        std::fs::create_dir(&work).unwrap();
        let original = "First line\r\nSecond line\n\"quotes\" %PATH% !EXPAND_ME! & echo UNEXPECTED | > injected.txt < nul ^ Unicode café 🧪";
        let binary = batch.canonicalize().unwrap();
        let args = adapters::argv_for_binary("opencode", original, "model/name", &binary).unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            Command::new(&binary).args(&args[1..]).current_dir(&work).output(),
        ).await.unwrap().unwrap();
        assert!(output.status.success(), "batch failed: {}", String::from_utf8_lossy(&output.stderr));
        let received: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(received, args[1..]);
        let task = received.last().unwrap();
        let json = task.strip_prefix("Decode this JSON string as the task text, preserving escaped newlines: ").unwrap();
        assert_eq!(serde_json::from_str::<String>(json).unwrap(), original);
        assert!(!work.join("injected.txt").exists());
    }.await;
    let _ = std::fs::remove_dir_all(dir);
    result
}
