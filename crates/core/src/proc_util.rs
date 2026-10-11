//! Helpers for running short-lived helper processes (help, version, list-devices).
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn command(program: &Path) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        c.creation_flags(CREATE_NO_WINDOW);
    }
    if let Some(d) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
        c.current_dir(d);
    }
    c.kill_on_drop(true);
    c
}

/// Run and capture stdout+stderr (combined) with a timeout.
pub async fn run_capture(program: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut c = command(program);
    c.args(args).stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(timeout, c.output())
        .await
        .context("timeout")?
        .with_context(|| format!("failed to run {}", program.display()))?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push('\n');
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(s)
}
