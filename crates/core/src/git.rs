//! Git integration for barrel workspaces.
//!
//! This module provides git worktree management, allowing barrel to create
//! isolated working directories for different branches.
//!
//! # Worktree Workflow
//!
//! ```bash
//! barrel -w feat/auth    # Create worktree + launch workspace
//! barrel -w feat/auth -k # Kill workspace + optionally prune worktree
//! ```
//!
//! Worktrees are created as siblings to the main repository:
//! ```text
//! ~/code/myproject/              # main repo
//! ~/code/myproject-feat-auth/    # worktree for feat/auth
//! ~/code/myproject-fix-bug/      # worktree for fix/bug
//! ```

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

/// Result of ensuring a worktree exists.
#[derive(Debug)]
pub struct WorktreeInfo {
    /// Path to the worktree directory
    pub path: PathBuf,
    /// Branch name
    pub branch: String,
    /// Whether the worktree was newly created
    pub created: bool,
    /// Whether the branch was newly created
    pub branch_created: bool,
}

/// Check if we're inside a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the repository root directory.
pub fn repo_root(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .context("Failed to execute git")?;

    if !output.status.success() {
        bail!("Not a git repository");
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

/// Get the repository name (directory name of the repo root).
pub fn repo_name(path: &Path) -> Result<String> {
    let root = repo_root(path)?;
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .context("Could not determine repository name")
}

/// Check if a branch exists locally.
pub fn branch_exists_local(path: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a branch exists on a remote.
pub fn branch_exists_remote(path: &Path, branch: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["branch", "-r", "--list", &format!("*/{}", branch)])
        .current_dir(path)
        .output()
        .ok()?;

    if output.status.success() {
        let remotes = String::from_utf8_lossy(&output.stdout);
        let remote = remotes.lines().next()?.trim();
        if !remote.is_empty() {
            return Some(remote.to_string());
        }
    }
    None
}

/// Get the current branch name.
pub fn current_branch(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .context("Failed to get current branch")?;

    if !output.status.success() {
        bail!("Failed to get current branch");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the default branch (main or master).
pub fn default_branch(path: &Path) -> Result<String> {
    // Try to get from remote HEAD
    let output = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
        .current_dir(path)
        .output();

    if let Ok(output) = output
        && output.status.success()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Strip "origin/" prefix
        if let Some(name) = branch.strip_prefix("origin/") {
            return Ok(name.to_string());
        }
    }

    // Fallback: check if main or master exists
    if branch_exists_local(path, "main") {
        return Ok("main".to_string());
    }
    if branch_exists_local(path, "master") {
        return Ok("master".to_string());
    }

    // Last resort: use current branch
    current_branch(path)
}

/// List all worktrees for a repository.
pub fn list_worktrees(path: &Path) -> Result<Vec<(PathBuf, String)>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(path)
        .output()
        .context("Failed to list worktrees")?;

    if !output.status.success() {
        bail!("Failed to list worktrees");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;

    for line in stdout.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/")
            && let Some(path) = current_path.take()
        {
            worktrees.push((path, branch.to_string()));
        }
    }

    Ok(worktrees)
}

/// Find existing worktree for a branch.
pub fn find_worktree(path: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let worktrees = list_worktrees(path)?;
    Ok(worktrees
        .into_iter()
        .find(|(_, b)| b == branch)
        .map(|(p, _)| p))
}

/// Convert branch name to a valid directory name.
fn branch_to_dirname(branch: &str) -> String {
    branch.replace(['/', '\\'], "-")
}

/// Ensure a worktree exists for a branch, creating if necessary.
///
/// If the branch doesn't exist, it will be created from the default branch.
/// The worktree is created as a sibling directory to the repository.
pub fn ensure_worktree(path: &Path, branch: &str) -> Result<WorktreeInfo> {
    let repo_root = repo_root(path)?;
    let repo_name = repo_name(path)?;

    // Check if worktree already exists for this branch
    if let Some(existing_path) = find_worktree(path, branch)? {
        // Verify the worktree directory actually exists
        if existing_path.exists() {
            return Ok(WorktreeInfo {
                path: existing_path,
                branch: branch.to_string(),
                created: false,
                branch_created: false,
            });
        } else {
            // Worktree reference exists but directory is gone - prune stale references
            prune_worktrees(path)?;
        }
    }

    // Determine worktree path (sibling to repo)
    let worktree_name = format!("{}-{}", repo_name, branch_to_dirname(branch));
    let worktree_path = repo_root
        .parent()
        .context("Repository has no parent directory")?
        .join(&worktree_name);

    // Check if branch exists
    let branch_exists = branch_exists_local(path, branch);
    let remote_branch = branch_exists_remote(path, branch);
    let branch_created;

    if branch_exists {
        // Branch exists locally, create worktree
        branch_created = false;
        let status = Command::new("git")
            .args(["worktree", "add", worktree_path.to_str().unwrap(), branch])
            .current_dir(&repo_root)
            .status()
            .context("Failed to create worktree")?;

        if !status.success() {
            bail!("Failed to create worktree for branch '{}'", branch);
        }
    } else if let Some(remote) = remote_branch {
        // Branch exists on remote, track it
        branch_created = false;
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "--track",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
                &remote,
            ])
            .current_dir(&repo_root)
            .status()
            .context("Failed to create worktree")?;

        if !status.success() {
            bail!(
                "Failed to create worktree tracking remote branch '{}'",
                remote
            );
        }
    } else {
        // Branch doesn't exist, create it from default branch
        branch_created = true;
        let base = default_branch(path)?;
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
                &base,
            ])
            .current_dir(&repo_root)
            .status()
            .context("Failed to create worktree")?;

        if !status.success() {
            bail!(
                "Failed to create worktree with new branch '{}' from '{}'",
                branch,
                base
            );
        }
    }

    // Symlink barrel.yaml if it exists in main repo but not in worktree
    let main_manifest = repo_root.join("barrel.yaml");
    let worktree_manifest = worktree_path.join("barrel.yaml");
    if main_manifest.exists() && !worktree_manifest.exists() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&main_manifest, &worktree_manifest).ok();
        }
    }

    Ok(WorktreeInfo {
        path: worktree_path,
        branch: branch.to_string(),
        created: true,
        branch_created,
    })
}

/// Remove a worktree.
///
/// If `force` is true, removes even if there are uncommitted changes.
pub fn remove_worktree(path: &Path, branch: &str, force: bool) -> Result<bool> {
    let worktree_path = match find_worktree(path, branch)? {
        Some(p) => p,
        None => return Ok(false),
    };

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path.to_str().unwrap());

    let status = Command::new("git")
        .args(&args)
        .current_dir(path)
        .status()
        .context("Failed to remove worktree")?;

    Ok(status.success())
}

/// Prune stale worktree references.
pub fn prune_worktrees(path: &Path) -> Result<()> {
    Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(path)
        .status()
        .context("Failed to prune worktrees")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_to_dirname() {
        assert_eq!(branch_to_dirname("feat/auth"), "feat-auth");
        assert_eq!(branch_to_dirname("fix/bug-123"), "fix-bug-123");
        assert_eq!(branch_to_dirname("main"), "main");
    }
}
