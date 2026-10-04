//! Persistent task graphs and monitoring for installed coding CLIs.
mod adapters;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_LOG: usize = 32_000;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
static IDS: AtomicU64 = AtomicU64::new(0);
static INSTANCE: OnceLock<Arc<Orchestrator>> = OnceLock::new();
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn default_parallel() -> usize {
    2
}
fn default_timeout() -> u64 {
    600
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub id: String,
    pub task: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub title: String,
    pub working_dir: String,
    pub tasks: Vec<TaskSpec>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    Succeeded,
    Failed,
    Blocked,
    Cancelled,
    Interrupted,
}
impl Status {
    fn terminal(&self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub spec: TaskSpec,
    pub status: Status,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub log: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub plan: Plan,
    pub created_at: u64,
    pub status: Status,
    pub jobs: Vec<Job>,
}
#[derive(Clone, Debug, Serialize)]
pub struct AgentAvailability {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeStatus {
    pub ready: bool,
    pub message: String,
    pub agents: Vec<AgentAvailability>,
}

pub fn runtime_status() -> RuntimeStatus {
        let agents: Vec<_> = adapters::adapters()
            .into_iter()
            .map(|a| AgentAvailability {
                id: a.id.into(),
                name: a.name.into(),
                available: crate::prefs::find_on_path(a.binary).is_some(),
                enabled: enabled(a.id),
            })
            .collect();
        let ready = agents.iter().any(|a| a.available && a.enabled);
        RuntimeStatus {
            ready,
            agents,
            message: if ready {
                "Installed agents use their existing logins and permissions. Tasks edit this project directly.".into()
            } else {
                "No enabled coding agents found. Install and log in to an agent, then rescan or enable it in Settings.".into()
            },
        }
}

pub struct Orchestrator {
    home: PathBuf,
    runs: Mutex<BTreeMap<String, Run>>,
    slots: Arc<tokio::sync::Semaphore>,
    processes: Mutex<BTreeMap<(String, String), u32>>,
}

pub fn global() -> Arc<Orchestrator> {
    INSTANCE
        .get_or_init(|| {
            Arc::new(Orchestrator::new(
                crate::paths::comrade_home().join("orchestration"),
            ))
        })
        .clone()
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn redact_error(message: String) -> String {
    let secrets: Vec<_> = adapters::adapters()
        .into_iter()
        .flat_map(|a| a.credentials.iter().copied())
        .filter_map(|key| std::env::var(key).ok())
        .filter(|v| !v.is_empty())
        .collect();
    adapters::redact(message, &secrets)
}

pub fn validate(plan: &Plan) -> Result<(), String> {
    if plan.title.trim().is_empty() || plan.title.len() > 200 || plan.working_dir.trim().is_empty()
    {
        return Err("Provide a title and project directory.".into());
    }
    if plan.tasks.is_empty() || plan.tasks.len() > 16 {
        return Err("A plan needs 1–16 tasks.".into());
    }
    if !(1..=3).contains(&plan.max_parallel) || !(30..=1800).contains(&plan.timeout_secs) {
        return Err("Use 1–3 parallel jobs and a 30–1800 second deadline.".into());
    }
    let mut ids = BTreeSet::new();
    for t in &plan.tasks {
        if !valid_id(&t.id) || !ids.insert(t.id.to_lowercase()) {
            return Err(
                "Task ids must be unique (ignoring case), with only letters, digits, '-' and '_'."
                    .into(),
            );
        }
        if t.task.trim().is_empty() || t.task.len() > 16_000 {
            return Err("Each task needs a prompt of at most 16000 bytes.".into());
        }
        if !t.agent.is_empty() {
            adapters::adapter(&t.agent)?;
        }
        if t.model.len() > 160
            || t.model.starts_with('-')
            || t.model.chars().any(|c| c.is_control())
        {
            return Err("Invalid model id.".into());
        }
    }
    for t in &plan.tasks {
        let mut unique = BTreeSet::new();
        for dep in &t.depends_on {
            if dep == &t.id || !plan.tasks.iter().any(|x| &x.id == dep) || !unique.insert(dep) {
                return Err(format!("Invalid dependency for {}: {dep}", t.id));
            }
        }
    }
    let mut done = BTreeSet::new();
    loop {
        let before = done.len();
        for t in &plan.tasks {
            if t.depends_on.iter().all(|d| done.contains(d)) {
                done.insert(t.id.clone());
            }
        }
        if done.len() == plan.tasks.len() {
            return Ok(());
        }
        if before == done.len() {
            return Err("Task dependencies contain a cycle.".into());
        }
    }
}

fn enabled(id: &str) -> bool {
    let prefs = crate::prefs::load().coding;
    prefs.agents.is_empty() || prefs.agents.iter().any(|s| s == id)
}

// Each worker owns a process group; stop includes CLI subprocesses.
fn stop_process(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}
struct Worker<'a> {
    manager: &'a Orchestrator,
    key: (String, String),
    pid: u32,
}
impl Drop for Worker<'_> {
    fn drop(&mut self) {
        let mut processes = self.manager.processes.lock().unwrap();
        stop_process(self.pid);
        processes.remove(&self.key);
    }
}
impl Orchestrator {
    pub fn new(home: PathBuf) -> Self {
        let mut runs: BTreeMap<String, Run> = std::fs::read(home.join("runs.json"))
            .ok()
            .and_then(|b| {
                let mut data: serde_json::Value = serde_json::from_slice(&b).ok()?;
                for run in data.as_object_mut()?.values_mut() {
                    if let Some(plan) = run.get_mut("plan").and_then(|p| p.as_object_mut()) {
                        plan.remove("network");
                    }
                }
                serde_json::from_value(data).ok()
            })
            .unwrap_or_default();
        // History never resumes processes or authorizes killing stored process ids.
        runs.retain(|id, r| {
            valid_id(id)
                && id == &r.id
                && validate(&r.plan).is_ok()
                && r.jobs.len() == r.plan.tasks.len()
                && r.jobs
                    .iter()
                    .zip(&r.plan.tasks)
                    .all(|(j, t)| j.spec.id == t.id && valid_id(&j.spec.id))
        });
        for r in runs.values_mut() {
            for j in &mut r.jobs {
                if !j.status.terminal() {
                    j.status = Status::Interrupted;
                    j.finished_at = Some(now());
                    j.error = Some("Comrade restarted; the job was not resumed.".into());
                }
            }
            if !r.status.terminal() {
                r.status = Status::Interrupted;
            }
        }
        Self {
            home,
            runs: Mutex::new(runs),
            slots: Arc::new(tokio::sync::Semaphore::new(3)),
            processes: Mutex::new(BTreeMap::new()),
        }
    }
    fn ensure_private_home(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.home).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.home, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn persist(&self, runs: &BTreeMap<String, Run>) -> Result<(), String> {
        self.ensure_private_home()?;
        let path = self.home.join("runs.json.tmp");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(runs).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(path, self.home.join("runs.json")).map_err(|e| e.to_string())
    }
    pub fn list(&self) -> Vec<Run> {
        let mut runs: Vec<_> = self.runs.lock().unwrap().values().cloned().collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        runs
    }
    pub fn get(&self, id: &str) -> Result<Run, String> {
        self.runs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| "Run not found.".into())
    }
    fn update(&self, run_id: &str, job_id: &str, status: Status, error: Option<String>) {
        let mut runs = self.runs.lock().unwrap();
        if let Some(run) = runs.get_mut(run_id) {
            if let Some(job) = run.jobs.iter_mut().find(|j| j.spec.id == job_id) {
                // A cancelled task must never be revived by a racing completion.
                if job.status == Status::Cancelled {
                    return;
                }
                job.status = status.clone();
                job.error = error.map(redact_error);
                if status == Status::Running {
                    job.started_at = Some(now());
                }
                if status.terminal() {
                    job.finished_at = Some(now());
                }
            }
            settle(run);
        }
        let _ = self.persist(&runs);
    }
    fn log(&self, run_id: &str, job_id: &str, text: &str) {
        if let Some(run) = self.runs.lock().unwrap().get_mut(run_id) {
            if let Some(job) = run.jobs.iter_mut().find(|j| j.spec.id == job_id) {
                job.log.push_str(text);
                if job.log.len() > MAX_LOG {
                    let mut cut = job.log.len() - MAX_LOG;
                    while !job.log.is_char_boundary(cut) {
                        cut += 1;
                    }
                    job.log.drain(..cut);
                }
            }
        }
    }
    pub async fn runtime(&self) -> RuntimeStatus {
        runtime_status()
    }
    pub async fn start(self: &Arc<Self>, mut plan: Plan) -> Result<Run, String> {
        validate(&plan)?;
        let runtime = self.runtime().await;
        if !runtime.ready {
            return Err(runtime.message);
        }
        let prefs = crate::prefs::load().coding;
        for task in &mut plan.tasks {
            if task.agent.is_empty() {
                task.agent = runtime
                    .agents
                    .iter()
                    .find(|a| a.available && a.enabled && a.id == prefs.default)
                    .or_else(|| runtime.agents.iter().find(|a| a.available && a.enabled))
                    .map(|a| a.id.clone())
                    .ok_or("No enabled agents are available on this machine.")?;
            }
            if !runtime
                .agents
                .iter()
                .any(|a| a.id == task.agent && a.available && a.enabled)
            {
                return Err(format!(
                    "{} is disabled or unavailable on this machine.",
                    task.agent
                ));
            }
        }
        let path = PathBuf::from(&plan.working_dir);
        if !path.is_absolute() || !path.is_dir() {
            return Err("Choose an existing absolute project directory.".into());
        }
        plan.working_dir = path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        let id = format!(
            "{:x}-{:x}-{:x}",
            now(),
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        );
        self.ensure_private_home()?;
        let run = Run {
            id: id.clone(),
            created_at: now(),
            status: Status::Queued,
            jobs: plan
                .tasks
                .iter()
                .cloned()
                .map(|spec| Job {
                    spec,
                    status: Status::Queued,
                    started_at: None,
                    finished_at: None,
                    log: String::new(),
                    error: None,
                })
                .collect(),
            plan,
        };
        {
            let mut runs = self.runs.lock().unwrap();
            if runs.len() >= 100 {
                return Err("Run history is full (100 runs). Archive the orchestration folder before starting more work.".into());
            }
            runs.insert(id.clone(), run.clone());
            if let Err(e) = self.persist(&runs) {
                runs.remove(&id);
                return Err(format!(
                    "Could not persist the run; no jobs were started: {e}"
                ));
            }
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.schedule(id).await;
        });
        Ok(run)
    }
    async fn schedule(self: Arc<Self>, id: String) {
        let mut pending = tokio::task::JoinSet::new();
        loop {
            let Ok(run) = self.get(&id) else {
                return;
            };
            for job in run.jobs.iter().filter(|j| j.status == Status::Queued) {
                if job.spec.depends_on.iter().any(|dep| {
                    run.jobs.iter().any(|j| {
                        &j.spec.id == dep && j.status.terminal() && j.status != Status::Succeeded
                    })
                }) {
                    self.update(
                        &id,
                        &job.spec.id,
                        Status::Blocked,
                        Some("A dependency did not succeed.".into()),
                    );
                }
            }
            let Ok(run) = self.get(&id) else {
                return;
            };
            if run.status.terminal() {
                break;
            }
            let running = run
                .jobs
                .iter()
                .filter(|j| j.status == Status::Running)
                .count();
            for job in run
                .jobs
                .iter()
                .filter(|j| {
                    j.status == Status::Queued
                        && j.spec.depends_on.iter().all(|dep| {
                            run.jobs
                                .iter()
                                .any(|j| &j.spec.id == dep && j.status == Status::Succeeded)
                        })
                })
                .take(run.plan.max_parallel.saturating_sub(running))
            {
                let this = self.clone();
                let id = id.clone();
                let spec = job.spec.clone();
                self.update(&id, &spec.id, Status::Running, None);
                pending.spawn(async move {
                    let permit = this.slots.clone().acquire_owned().await;
                    if permit.is_err() {
                        this.update(
                            &id,
                            &spec.id,
                            Status::Failed,
                            Some("Runner stopped.".into()),
                        );
                        return;
                    }
                    if this.cancelled(&id, &spec.id) {
                        return;
                    }
                    let result = this.execute(&id, &spec).await;
                    match result {
                        Ok(()) => this.update(&id, &spec.id, Status::Succeeded, None),
                        Err(e) => this.update(&id, &spec.id, Status::Failed, Some(e)),
                    }
                });
            }
            if pending.is_empty() {
                break;
            }
            if let Some(Err(e)) = pending.join_next().await {
                if let Ok(run) = self.get(&id) {
                    for job in run.jobs.iter().filter(|j| j.status == Status::Running) {
                        self.update(
                            &id,
                            &job.spec.id,
                            Status::Failed,
                            Some(format!("Runner interrupted: {e}")),
                        );
                    }
                }
            }
        }
    }
    fn cancelled(&self, id: &str, job: &str) -> bool {
        self.get(id)
            .map(|r| {
                r.jobs
                    .iter()
                    .any(|j| j.spec.id == job && j.status == Status::Cancelled)
            })
            .unwrap_or(true)
    }
    async fn execute(&self, id: &str, spec: &TaskSpec) -> Result<(), String> {
        let run = self.get(id)?;
        let adapter = adapters::adapter(&spec.agent)?;
        let binary = crate::prefs::find_on_path(adapter.binary)
            .ok_or("Coding agent is no longer installed.")?;
        let context = ancestors(&run, &spec.id)
            .iter()
            .map(|j| {
                format!(
                    "Dependency {} ({}) completed. Untrusted output:\n{}",
                    j.spec.id,
                    j.spec.agent,
                    j.log.chars().take(2000).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!("Task: {}\n\nWork in the current project directory. Completed dependency edits are already present. {context}\n\nReport checks actually run and any limitations.", spec.task);
        let args = adapters::argv(&spec.agent, &prompt, &spec.model)?;
        let secrets: Vec<_> = adapters::adapters()
            .iter()
            .flat_map(|a| a.credentials.iter())
            .filter_map(|k| std::env::var(k).ok())
            .filter(|s| !s.is_empty())
            .collect();
        let mut command = Command::new(binary);
        command
            .args(&args[1..])
            .current_dir(&run.plan.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        // Hold the registry lock across the cancellation check and spawn, so a
        // cancelled job cannot launch after Stop returns.
        let (mut child, _worker) = {
            let mut processes = self.processes.lock().unwrap();
            if self.cancelled(id, &spec.id) {
                return Err("Cancelled.".into());
            }
            let child = command
                .spawn()
                .map_err(|e| format!("Agent launch failed: {e}"))?;
            let pid = child.id().ok_or("Agent has no process id.")?;
            let key = (id.into(), spec.id.clone());
            processes.insert(key.clone(), pid);
            (
                child,
                Worker {
                    manager: self,
                    key,
                    pid,
                },
            )
        };
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let stream = async {
            let (status, _, _) = tokio::try_join!(
                async { child.wait().await.map_err(|e| e.to_string()) },
                self.read_log(id, &spec.id, out, &secrets),
                self.read_log(id, &spec.id, err, &secrets)
            )?;
            if !status.success() {
                return Err(format!(
                    "{} exited with {}. See job logs.",
                    adapter.name, status
                ));
            }
            Ok(())
        };
        let monitor = async {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if self.cancelled(id, &spec.id) {
                    return Err("Cancelled.".into());
                }
            }
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(run.plan.timeout_secs), stream) =>
                result.map_err(|_| format!("Deadline exceeded ({} seconds).", run.plan.timeout_secs))?,
            result = monitor => result,
        }
    }
    async fn read_log<R: tokio::io::AsyncRead + Unpin>(
        &self,
        id: &str,
        job: &str,
        mut reader: R,
        secrets: &[String],
    ) -> Result<(), String> {
        // Keep a suffix between reads so a secret split over pipe boundaries is
        // still redacted. Cap total bytes independently of the visible log tail.
        let hold = secrets.iter().map(|s| s.len()).max().unwrap_or(0).max(4);
        let mut pending = Vec::new();
        let mut total = 0;
        let mut buf = [0u8; 4096];
        loop {
            let n = reader.read(&mut buf).await.map_err(|e| e.to_string())?;
            total += n;
            if total > MAX_OUTPUT / 2 {
                return Err("Agent output budget exceeded.".into());
            }
            pending.extend_from_slice(&buf[..n]);
            let mut cut = if n == 0 {
                pending.len()
            } else {
                pending.len().saturating_sub(hold)
            };
            // Never cut a secret at the emission boundary.
            for s in secrets {
                for pos in pending
                    .windows(s.len())
                    .enumerate()
                    .filter(|(_, w)| *w == s.as_bytes())
                    .map(|(i, _)| i)
                {
                    if pos < cut && pos + s.len() > cut {
                        cut = pos;
                    }
                }
            }
            while cut > 0 && cut < pending.len() && pending[cut] & 0xc0 == 0x80 {
                cut -= 1;
            }
            if cut > 0 {
                let text = adapters::redact(
                    String::from_utf8_lossy(&pending[..cut]).into_owned(),
                    secrets,
                );
                self.log(id, job, &text);
                pending.drain(..cut);
            }
            if n == 0 {
                break;
            }
        }
        Ok(())
    }
    pub async fn cancel(&self, id: &str, job_id: Option<&str>) -> Result<Run, String> {
        {
            let processes = self.processes.lock().unwrap();
            let mut runs = self.runs.lock().unwrap();
            let run = runs.get_mut(id).ok_or("Run not found.")?;
            if job_id.is_some_and(|job| !run.jobs.iter().any(|j| j.spec.id == job)) {
                return Err("Job not found.".into());
            }
            for job in &mut run.jobs {
                if job_id.map(|s| s == job.spec.id).unwrap_or(true) && !job.status.terminal() {
                    job.status = Status::Cancelled;
                    job.finished_at = Some(now());
                    if let Some(pid) = processes.get(&(id.into(), job.spec.id.clone())) {
                        stop_process(*pid);
                    }
                }
            }
            settle(run);
            self.persist(&runs)?;
        }
        // Wait for the worker to drain/reap and release its process registration.
        let stopped = async {
            loop {
                let pending = self
                    .processes
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|(r, j)| r == id && job_id.map(|s| s == j).unwrap_or(true));
                if !pending {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), stopped)
            .await
            .map_err(|_| "Agent stop has not finished; check job logs.".to_string())?;
        self.get(id)
    }
}
fn settle(run: &mut Run) {
    run.status = if run.jobs.iter().all(|j| j.status.terminal()) {
        if run.jobs.iter().all(|j| j.status == Status::Succeeded) {
            Status::Succeeded
        } else if run.jobs.iter().any(|j| {
            matches!(
                j.status,
                Status::Failed | Status::Blocked | Status::Interrupted
            )
        }) {
            Status::Failed
        } else {
            Status::Cancelled
        }
    } else if run.jobs.iter().any(|j| j.status == Status::Running) {
        Status::Running
    } else {
        Status::Queued
    };
}
fn ancestors<'a>(run: &'a Run, job_id: &str) -> Vec<&'a Job> {
    fn visit<'a>(run: &'a Run, id: &str, seen: &mut BTreeSet<String>, out: &mut Vec<&'a Job>) {
        if let Some(job) = run.jobs.iter().find(|j| j.spec.id == id) {
            for dep in &job.spec.depends_on {
                if seen.insert(dep.clone()) {
                    visit(run, dep, seen, out);
                    if let Some(j) = run.jobs.iter().find(|j| &j.spec.id == dep) {
                        out.push(j);
                    }
                }
            }
        }
    }
    let mut out = vec![];
    visit(run, job_id, &mut BTreeSet::new(), &mut out);
    out
}

#[cfg(test)]
mod tests;
