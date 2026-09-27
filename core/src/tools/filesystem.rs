/** Filesystem tools: all FS access goes through here, never direct LLM access. */
use std::future::Future;
use std::pin::Pin;
use std::path::{Path, PathBuf};

use super::types::{Risk, Tool, ToolContext, ToolResult};

const BLOCKED_SEGMENTS: &[&str] = &[".ssh", ".gnupg", ".pki"];
const BLOCKED_FILES: &[&str] = &[".env"];

fn resolve_safe(cwd: &Path, target: &str) -> Option<PathBuf> {
    let resolved = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        cwd.join(target)
    };
    // Lexical normalization (no symlink resolution — best-effort guard).
    let mut clean = PathBuf::new();
    for comp in resolved.components() {
        use std::path::Component::*;
        match comp {
            Prefix(p) => clean.push(p.as_os_str()),
            RootDir => clean.push("/"),
            CurDir => {}
            ParentDir => {
                clean.pop();
            }
            Normal(c) => clean.push(c),
        }
    }
    let blocked_segment = clean.components().any(|c| {
        c.as_os_str().to_str().is_some_and(|s| BLOCKED_SEGMENTS.contains(&s))
    });
    let blocked_file = clean
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| BLOCKED_FILES.contains(&n));
    if blocked_segment || blocked_file {
        return None;
    }
    Some(clean)
}

macro_rules! tool {
    ($name:ident, $tool_name:literal, $desc:literal, $params:expr, $risk:expr, $run:path) => {
        pub struct $name;
        impl Tool for $name {
            fn name(&self) -> &'static str {
                $tool_name
            }
            fn description(&self) -> &'static str {
                $desc
            }
            fn parameters(&self) -> serde_json::Value {
                $params
            }
            fn risk(&self, _args: &serde_json::Value) -> Risk {
                $risk
            }
            fn execute<'a>(
                &'a self,
                args: &'a serde_json::Value,
                ctx: &'a ToolContext,
            ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
                Box::pin($run(args, ctx))
            }
        }
    };
}

async fn run_list(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let dir = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let Some(resolved) = resolve_safe(&ctx.cwd, dir) else {
        return ToolResult::fail("BLOCKED_PATH", "Access to that path is blocked.");
    };
    let mut entries = match tokio::fs::read_dir(&resolved).await {
        Ok(e) => e,
        Err(e) => return ToolResult::fail("READ_FAILED", format!("Cannot list directory: {e}")),
    };
    let mut out = Vec::new();
    loop {
        match entries.next_entry().await {
            Ok(Some(e)) => {
                let file_type = e.file_type().await.ok();
                let kind = match file_type {
                    Some(t) if t.is_dir() => "dir",
                    Some(t) if t.is_file() => "file",
                    _ => "other",
                };
                out.push(serde_json::json!({
                    "name": e.file_name().to_string_lossy(),
                    "type": kind,
                }));
            }
            Ok(None) => break,
            Err(e) => return ToolResult::fail("READ_FAILED", format!("Cannot list directory: {e}")),
        }
    }
    ToolResult::ok(serde_json::Value::Array(out))
}

async fn run_read(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let Some(resolved) = resolve_safe(&ctx.cwd, path) else {
        return ToolResult::fail("BLOCKED_PATH", "Access to that path is blocked.");
    };
    let meta = match tokio::fs::metadata(&resolved).await {
        Ok(m) => m,
        Err(e) => return ToolResult::fail("READ_FAILED", format!("Cannot read file: {e}")),
    };
    if !meta.is_file() {
        return ToolResult::fail("NOT_A_FILE", "Path is not a file.");
    }
    let max_bytes = args.get("maxBytes").and_then(|m| m.as_u64()).unwrap_or(200_000).min(1_000_000) as usize;
    let bytes = match tokio::fs::read(&resolved).await {
        Ok(b) => b,
        Err(e) => return ToolResult::fail("READ_FAILED", format!("Cannot read file: {e}")),
    };
    let truncated = bytes.len() > max_bytes;
    let content = String::from_utf8_lossy(&bytes[..bytes.len().min(max_bytes)]).to_string();
    ToolResult::ok(serde_json::json!({
        "path": resolved.to_string_lossy(),
        "size": meta.len(),
        "truncated": truncated,
        "content": content,
    }))
}

async fn run_write(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let Some(resolved) = resolve_safe(&ctx.cwd, path) else {
        return ToolResult::fail("BLOCKED_PATH", "Access to that path is blocked.");
    };
    if let Some(parent) = resolved.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return ToolResult::fail("WRITE_FAILED", format!("Cannot create parent dir: {e}"));
        }
    }
    match tokio::fs::write(&resolved, content).await {
        Ok(()) => ToolResult::ok(serde_json::json!({
            "path": resolved.to_string_lossy(),
            "bytes": content.len(),
        })),
        Err(e) => ToolResult::fail("WRITE_FAILED", format!("Cannot write file: {e}")),
    }
}

async fn run_exists(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let Some(resolved) = resolve_safe(&ctx.cwd, path) else {
        return ToolResult::fail("BLOCKED_PATH", "Access to that path is blocked.");
    };
    ToolResult::ok(serde_json::json!({
        "path": resolved.to_string_lossy(),
        "exists": tokio::fs::metadata(&resolved).await.is_ok(),
    }))
}

async fn run_search(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("").to_lowercase();
    if query.is_empty() {
        return ToolResult::fail("EMPTY_QUERY", "No search query provided.");
    }
    let dir = args.get("dir").and_then(|d| d.as_str()).unwrap_or(".");
    let Some(root) = resolve_safe(&ctx.cwd, dir) else {
        return ToolResult::fail("BLOCKED_PATH", "Access to that path is blocked.");
    };
    let mut hits: Vec<String> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root, 0)];
    while let Some((dir, depth)) = stack.pop() {
        if hits.len() >= 50 || depth > 6 {
            continue;
        }
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        loop {
            if hits.len() >= 50 {
                break;
            }
            match entries.next_entry().await {
                Ok(Some(e)) => {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name == "node_modules" || name == ".git" || name == "target" {
                        continue;
                    }
                    let full = e.path();
                    if name.to_lowercase().contains(&query) {
                        let rel = full.strip_prefix(&ctx.cwd).unwrap_or(&full).to_string_lossy().to_string();
                        hits.push(rel);
                    }
                    if full.is_dir() {
                        stack.push((full, depth + 1));
                    }
                }
                _ => break,
            }
        }
    }
    ToolResult::ok(serde_json::json!({ "query": args.get("query"), "hits": hits }))
}

tool!(
    ListTool,
    "filesystem.list",
    "List directory entries (name + type). Path is relative to cwd or absolute.",
    serde_json::json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": "Directory to list (default .)" } },
    }),
    Risk::Safe,
    run_list
);

tool!(
    ReadTool,
    "filesystem.read",
    "Read a text file (max ~200KB, truncated beyond).",
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "maxBytes": { "type": "number" },
        },
        "required": ["path"],
    }),
    Risk::Safe,
    run_read
);

tool!(
    WriteTool,
    "filesystem.write",
    "Write/overwrite a text file. Requires user approval (dangerous).",
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "content": { "type": "string" },
        },
        "required": ["path", "content"],
    }),
    Risk::Dangerous,
    run_write
);

tool!(
    ExistsTool,
    "filesystem.exists",
    "Check whether a path exists.",
    serde_json::json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
    }),
    Risk::Safe,
    run_exists
);

tool!(
    SearchTool,
    "filesystem.search",
    "Search file names under a directory (substring match, max 50 hits).",
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "dir": { "type": "string", "description": "Directory to search (default .)" },
        },
        "required": ["query"],
    }),
    Risk::Safe,
    run_search
);
