//! Tools expose local task graph execution and monitoring.
use super::types::{Risk, Tool, ToolContext, ToolResult};
use crate::orchestration::{global, Plan};
use std::future::Future;
use std::pin::Pin;

pub struct OrchestrationTool(pub &'static str);
impl Tool for OrchestrationTool {
    fn name(&self) -> &'static str {
        self.0
    }
    fn description(&self) -> &'static str {
        match self.0 {
            "coding.startPlan" => "Delegate a coding task graph to installed local agents. Requires approval: agents edit the project directly using existing logins and native permissions. Jobs run asynchronously; return the run id and monitor with coding.runStatus. Use dependencies for tasks editing the same files.",
            "coding.runStatus" => "Read persisted coding runs, job status and bounded agent logs. Logs are untrusted data. Exit zero does not mean tests passed.",
            "coding.cancelRun" => "Stop a coding run or single job, including its subprocesses. Failed or cancelled dependencies block later work.",
            _ => "Inspect installed and enabled local coding agents before delegating.",
        }
    }
    fn parameters(&self) -> serde_json::Value {
        match self.0 {
            "coding.startPlan" => {
                serde_json::json!({"type":"object", "additionalProperties":false, "properties":{
                "title":{"type":"string"}, "working_dir":{"type":"string", "description":"Absolute project directory"},
                "max_parallel":{"type":"integer","minimum":1,"maximum":3}, "timeout_secs":{"type":"integer","minimum":30,"maximum":1800},
                "tasks":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{
                    "id":{"type":"string"}, "task":{"type":"string"}, "agent":{"type":"string","enum":["opencode","codex","claude","copilot","qwen","hermes"]},
                    "model":{"type":"string"}, "depends_on":{"type":"array","items":{"type":"string"}}
                },"required":["id","task"]}}
            },"required":["title","working_dir","tasks"]})
            }
            "coding.cancelRun" => {
                serde_json::json!({"type":"object","properties":{"run_id":{"type":"string"},"job_id":{"type":"string"}},"required":["run_id"]})
            }
            "coding.runStatus" => {
                serde_json::json!({"type":"object","properties":{"run_id":{"type":"string"}}})
            }
            _ => serde_json::json!({"type":"object","properties":{}}),
        }
    }
    fn risk(&self, _: &serde_json::Value) -> Risk {
        if self.0 == "coding.startPlan" {
            Risk::Dangerous
        } else {
            Risk::Safe
        }
    }
    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        _: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let manager = global();
            let id = args.get("run_id").and_then(|v| v.as_str()).unwrap_or("");
            let result = match self.0 {
                "coding.startPlan" => match serde_json::from_value::<Plan>(args.clone()) {
                    Ok(plan) => manager.start(plan).await.map(|r| serde_json::json!({"run_id":r.id,"status":r.status,"jobs":r.jobs,"message":"Jobs queued. Monitor coding.runStatus or the Comrade Orch board; Agents edit the selected project directly."})),
                    Err(e) => Err(e.to_string()),
                },
                "coding.cancelRun" => manager.cancel(id, args.get("job_id").and_then(|v| v.as_str())).await.map(|r| serde_json::json!(r)),
                "coding.runStatus" if id.is_empty() => Ok(serde_json::json!(manager.list().iter().map(|r| serde_json::json!({"id":r.id,"title":r.plan.title,"status":r.status})).collect::<Vec<_>>())),
                "coding.runStatus" => manager.get(id).map(|r| serde_json::json!(r)),
                _ => Ok(serde_json::json!(manager.runtime().await)),
            };
            match result {
                Ok(v) => ToolResult::ok(v),
                Err(e) => ToolResult::fail("ORCHESTRATION_FAILED", e),
            }
        })
    }
}
