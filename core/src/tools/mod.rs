pub mod browser;
pub mod browser_driver;
mod cdp_transport;
mod adblock;
pub mod coding;
pub mod computer;
pub mod filesystem;
pub mod provision;
pub mod terminal;
pub mod types;

use std::sync::Arc;

pub use types::{Risk, Tool, ToolContext, ToolResult};

/// Full tool registry handed to the agent loop.
pub fn build_tools() -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(terminal::TerminalTool),
        Arc::new(filesystem::ListTool),
        Arc::new(filesystem::ReadTool),
        Arc::new(filesystem::WriteTool),
        Arc::new(filesystem::ExistsTool),
        Arc::new(filesystem::SearchTool),
        Arc::new(computer::OpenApplicationTool),
        Arc::new(coding::CodingTool),
    ];
    for stub in computer::computer_stubs() {
        tools.push(Arc::new(stub));
    }
    for tool in browser::browser_tools() {
        tools.push(tool);
    }
    tools
}
