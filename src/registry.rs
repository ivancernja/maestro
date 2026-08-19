use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// One workspace: a git worktree plus the tmux session running its agent.
/// The on-disk shape is shared with the shell prototype, so registries carry over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub branch: String,
    pub repo: String,
    #[serde(rename = "repoPath")]
    pub repo_path: String,
    pub worktree: String,
    pub agent: String,
    /// Branch the worktree was created from. Defaulted for older entries.
    #[serde(default)]
    pub base: String,
    pub session: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MAESTRO_STATE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/maestro")
}

pub fn ws_dir() -> PathBuf {
    state_dir().join("ws")
}

pub fn ensure_dirs() -> Result<()> {
    fs::create_dir_all(ws_dir()).context("creating the state directory")?;
    Ok(())
}

/// Oldest first, so the sidebar order matches creation order.
pub fn load_all() -> Vec<Workspace> {
    let mut out: Vec<Workspace> = Vec::new();
    let Ok(entries) = fs::read_dir(ws_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(ws) = serde_json::from_str::<Workspace>(&text)
        {
            out.push(ws);
        }
    }
    out.sort_by_key(|w| w.created_at);
    out
}

pub fn exists(id: &str) -> bool {
    ws_dir().join(format!("{id}.json")).exists()
}

pub fn save(ws: &Workspace) -> Result<()> {
    ensure_dirs()?;
    let path = ws_dir().join(format!("{}.json", ws.id));
    fs::write(&path, serde_json::to_string_pretty(ws)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn delete(id: &str) -> Result<()> {
    let path = ws_dir().join(format!("{id}.json"));
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
