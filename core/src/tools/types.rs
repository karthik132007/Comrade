/** Shared tool types. Every tool returns a structured result — never panics. */
use std::future::Future;
use std::pin::Pin;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum ToolResult {
    Ok(serde_json::Value),
    Err(ToolError),
}

impl ToolResult {
    pub fn ok(data: serde_json::Value) -> Self {
        ToolResult::Ok(data)
    }

    pub fn fail(code: &str, message: impl Into<String>) -> Self {
        ToolResult::Err(ToolError { code: code.to_string(), message: message.into() })
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, ToolResult::Ok(_))
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ToolResult::Ok(data) => serde_json::json!({ "success": true, "data": data }),
            ToolResult::Err(e) => serde_json::json!({
                "success": false,
                "error": { "code": e.code, "message": e.message },
            }),
        }
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            ToolResult::Err(e) => Some(&e.message),
            ToolResult::Ok(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    Safe,
    Dangerous,
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub cwd: PathBuf,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> serde_json::Value;
    fn risk(&self, args: &serde_json::Value) -> Risk;
    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;
}
