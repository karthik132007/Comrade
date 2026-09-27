/** Task state tracked across the agent loop. */
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    AwaitingApproval,
    Done,
    Cancelled,
    Failed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::AwaitingApproval => "awaiting_approval",
            TaskStatus::Done => "done",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Active,
    Done,
    Failed,
    Skipped,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Active => "active",
            StepStatus::Done => "done",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStep {
    pub label: String,
    pub status: StepStatus,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub args: serde_json::Value,
    pub result_summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskState {
    pub id: String,
    pub user_request: String,
    pub intent: String,
    pub status: TaskStatus,
    pub current_step: String,
    pub steps: Vec<TaskStep>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub started_at: u64,
    pub updated_at: u64,
}

pub fn create_task(user_request: &str, intent: &str) -> TaskState {
    let now = now_ms();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    TaskState {
        id: format!("task-{now}-{n}"),
        user_request: user_request.to_string(),
        intent: intent.to_string(),
        status: TaskStatus::Pending,
        current_step: "created".into(),
        steps: vec![],
        tool_calls: vec![],
        result: None,
        error: None,
        started_at: now,
        updated_at: now,
    }
}

pub fn add_step(task: &mut TaskState, label: &str) {
    task.steps.push(TaskStep { label: label.to_string(), status: StepStatus::Active, detail: None });
    task.current_step = label.to_string();
    task.updated_at = now_ms();
}

pub fn finish_step(task: &mut TaskState, index: usize, status: StepStatus, detail: Option<String>) {
    if let Some(step) = task.steps.get_mut(index) {
        step.status = status;
        step.detail = detail;
    }
    task.updated_at = now_ms();
}
