use anyhow::{Result, anyhow};
use std::process::{Command, Stdio};

/// tmux hosts the agent processes. It is never shown: the app attaches to these
/// sessions inside its own pty, so agents survive quitting maestro entirely.
pub fn prefix() -> String {
    std::env::var("MAESTRO_SESSION_PREFIX").unwrap_or_else(|_| "mst-".into())
}

pub fn session_for(id: &str) -> String {
    format!("{}{}", prefix(), id)
}

fn tmux(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux").args(args).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "tmux {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn has_session(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn new_session(name: &str, cwd: &str, command: &str) -> Result<()> {
    tmux(&[
        "new-session",
        "-d",
        "-s",
        name,
        "-c",
        cwd,
        "-x",
        "200",
        "-y",
        "50",
        command,
    ])?;
    // No status line: the pane is rendered inside maestro's own chrome. Sessions
    // are deliberately allowed to die with their agent so a finished or crashed
    // agent reads as gone rather than idle.
    let _ = tmux(&["set-option", "-t", name, "status", "off"]);
    let _ = tmux(&["set-option", "-t", name, "remain-on-exit", "on"]);
    Ok(())
}

pub fn kill_session(name: &str) {
    let _ = tmux(&["kill-session", "-t", name]);
}

/// Panes are addressed by absolute id: omarchy's tmux.conf sets base-index 1,
/// so numeric targets like ":0.0" do not resolve.
pub fn first_pane(session: &str) -> Option<String> {
    let out = tmux(&["list-panes", "-t", session, "-F", "#{pane_id}"]).ok()?;
    out.lines().next().map(|s| s.trim().to_string())
}

/// remain-on-exit keeps a finished agent's last screen readable, so a live
/// session is not proof of a live agent — the pane's own dead flag is.
pub fn pane_dead(session: &str) -> bool {
    match tmux(&["list-panes", "-t", session, "-F", "#{pane_dead}"]) {
        Ok(out) => out.lines().next().map(|l| l.trim() == "1").unwrap_or(false),
        Err(_) => true,
    }
}

pub fn capture(pane: &str) -> Option<String> {
    tmux(&["capture-pane", "-p", "-t", pane]).ok()
}
