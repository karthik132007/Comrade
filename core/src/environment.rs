/** Environment context passed to the agent at task start. Minimal by design. */
use std::path::Path;

#[derive(Clone, Debug)]
pub struct EnvironmentContext {
    pub os: String,
    pub arch: String,
    pub hostname: String,
    pub cwd: String,
    pub git_repo: Option<String>,
    pub git_branch: Option<String>,
}

fn hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

async fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("git").args(args).current_dir(cwd).output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub async fn detect_environment(cwd: &Path) -> EnvironmentContext {
    let git_repo = git_output(cwd, &["rev-parse", "--show-toplevel"]).await;
    let git_branch = if git_repo.is_some() {
        git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await
    } else {
        None
    };
    // Kernel release for a more useful OS line (best-effort).
    let release = tokio::process::Command::new("uname")
        .arg("-r")
        .output()
        .await
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    EnvironmentContext {
        os: format!("{} {}", std::env::consts::OS, release).trim().to_string(),
        arch: std::env::consts::ARCH.to_string(),
        hostname: hostname(),
        cwd: cwd.to_string_lossy().to_string(),
        git_repo,
        git_branch,
    }
}

pub fn environment_prompt(ctx: &EnvironmentContext) -> String {
    format!(
        "OS: {} ({})\nHost: {}\nWorking directory: {}\nGit repo: {}\nGit branch: {}",
        ctx.os,
        ctx.arch,
        ctx.hostname,
        ctx.cwd,
        ctx.git_repo.as_deref().unwrap_or("none"),
        ctx.git_branch.as_deref().unwrap_or("n/a"),
    )
}
