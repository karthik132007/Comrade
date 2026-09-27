pub mod browser;
pub mod computer;
pub mod filesystem;
pub mod opencode;
pub mod terminal;
pub mod types;

use std::sync::Arc;

pub use types::{Risk, Tool, ToolContext, ToolResult};

/// Full tool registry handed to the agent loop.
pub fn build_tools(opencode_bin: &str) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(terminal::TerminalTool),
        Arc::new(filesystem::ListTool),
        Arc::new(filesystem::ReadTool),
        Arc::new(filesystem::WriteTool),
        Arc::new(filesystem::ExistsTool),
        Arc::new(filesystem::SearchTool),
        Arc::new(computer::OpenApplicationTool),
        Arc::new(opencode::OpenCodeTool { bin: opencode_bin.to_string() }),
    ];
    for stub in computer::computer_stubs() {
        tools.push(Arc::new(stub));
    }
    for stub in browser::browser_stubs() {
        tools.push(Arc::new(stub));
    }
    tools
}
