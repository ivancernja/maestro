use anyhow::{Context, Result, anyhow};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        return Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn repo_root(dir: &Path) -> Result<PathBuf> {
    let out = git(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Worktrees land at `../<repo>--<branch>`, the same convention as omarchy's
/// `ga` helper, so the two are interchangeable.
pub fn worktree_path(root: &Path, branch: &str) -> PathBuf {
    let repo = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let parent = root.parent().unwrap_or(Path::new("/tmp"));
    parent.join(format!("{repo}--{branch}"))
}

pub fn add_worktree(root: &Path, branch: &str, path: &Path) -> Result<()> {
    git(
        root,
        &["worktree", "add", "-b", branch, &path.to_string_lossy()],
    )?;
    Ok(())
}

pub fn current_branch(root: &Path) -> Option<String> {
    let out = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let name = out.trim().to_string();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name)
    }
}

pub fn is_clean(dir: &Path) -> bool {
    git(dir, &["status", "--porcelain"])
        .map(|out| out.trim().is_empty())
        .unwrap_or(false)
}

/// Commits on `branch` that the base does not have yet.
pub fn ahead_of(root: &Path, base: &str, branch: &str) -> u32 {
    git(root, &["rev-list", "--count", &format!("{base}..{branch}")])
        .ok()
        .and_then(|out| out.trim().parse().ok())
        .unwrap_or(0)
}

pub fn merge(root: &Path, branch: &str) -> Result<String> {
    git(root, &["merge", "--no-ff", "--no-edit", branch])
}

pub fn branch_exists(root: &Path, branch: &str) -> bool {
    git(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

pub fn remove_worktree(root: &Path, path: &Path) {
    let _ = git(
        root,
        &["worktree", "remove", &path.to_string_lossy(), "--force"],
    );
}

pub fn delete_branch(root: &Path, branch: &str) {
    let _ = git(root, &["branch", "-D", branch]);
}

/// Added and removed lines against HEAD, staged and unstaged together.
pub fn diffstat(worktree: &Path) -> (u32, u32) {
    let Ok(out) = git(worktree, &["diff", "--numstat", "HEAD"]) else {
        return (0, 0);
    };
    let mut added = 0;
    let mut removed = 0;
    for line in out.lines() {
        let mut cols = line.split_whitespace();
        let (Some(a), Some(r)) = (cols.next(), cols.next()) else {
            continue;
        };
        added += a.parse::<u32>().unwrap_or(0);
        removed += r.parse::<u32>().unwrap_or(0);
    }
    (added, removed)
}

/// Repositories to offer when creating a workspace. Existing worktrees are
/// skipped: their directory names carry the `--` marker.
pub fn find_repos(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, 0, max_depth, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, max_depth: usize, found: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            if !dir.to_string_lossy().contains("--") {
                found.push(dir.to_path_buf());
            }
            return;
        }
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            children.push(path);
        }
    }
    for child in children {
        walk(&child, depth + 1, max_depth, found);
    }
}
