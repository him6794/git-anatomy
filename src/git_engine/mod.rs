//! Git Engine: Extract commit history and file deltas using git2-rs
//!
//! This module directly reads the .git directory via libgit2 (git2-rs),
//! never invoking the system `git` command. It walks the commit DAG,
//! extracts metadata (hash, author, timestamp), and computes file-level
//! diffs (deltas) for each commit.

#![allow(dead_code)]

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository, Time};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ─── Data Types ──────────────────────────────────────────────────────────────

/// A single commit's metadata and associated file changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    /// Full 40-character SHA-1 hash
    pub hash: String,
    /// Short 7-character hash (for display)
    pub short_hash: String,
    /// Author name
    pub author_name: String,
    /// Author email
    pub author_email: String,
    /// Commit timestamp (Unix epoch seconds)
    pub timestamp: i64,
    /// UTC offset in minutes
    pub utc_offset: i32,
    /// Commit message (first line)
    pub summary: String,
    /// List of file paths modified in this commit
    pub file_deltas: Vec<FileDelta>,
}

/// A single file's change within a commit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDelta {
    /// File path relative to repo root
    pub path: String,
    /// Type of change (added, modified, deleted, renamed)
    pub change_type: ChangeType,
}

/// Classification of how a file was changed in a commit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Added => write!(f, "A"),
            ChangeType::Modified => write!(f, "M"),
            ChangeType::Deleted => write!(f, "D"),
            ChangeType::Renamed => write!(f, "R"),
            ChangeType::Untracked => write!(f, "?"),
        }
    }
}

// ─── Repository Operations ───────────────────────────────────────────────────

/// Open a git repository at the given path.
///
/// Uses `git2::Repository::open` which reads .git directly.
/// Falls back to `discover` if the path is a subdirectory.
pub fn open_repo(path: &Path) -> Result<Repository> {
    let repo = if path.join(".git").exists() {
        Repository::open(path)
            .with_context(|| format!("Failed to open repository at {}", path.display()))?
    } else {
        Repository::discover(path)
            .with_context(|| format!("Failed to discover repository from {}", path.display()))?
    };

    // Verify the repository is not bare
    if repo.is_bare() {
        anyhow::bail!("Repository at {} is bare; git-anatomy requires a working tree", path.display());
    }

    Ok(repo)
}

/// Extract the full commit history from a repository.
///
/// Walks the commit DAG starting from HEAD, extracting metadata and
/// file-level diffs for each commit. Uses a topological sort to ensure
/// we process commits in order from oldest to newest.
///
/// # Arguments
/// * `repo` - An open git2::Repository
/// * `max_commits` - Maximum number of commits to process (0 = all)
///
/// # Returns
/// A vector of `CommitRecord` sorted from oldest to newest.
pub fn extract_commit_history(repo: &Repository, max_commits: usize) -> Result<Vec<CommitRecord>> {
    // Resolve HEAD to get the starting commit
    let head = repo.head()
        .context("Failed to resolve HEAD. Ensure the repository has at least one commit.")?;
    let head_oid = head.target()
        .context("HEAD does not point to a commit.")?;

    // Create a revwalk to traverse the commit DAG
    let mut revwalk = repo.revwalk()
        .context("Failed to create revwalk iterator.")?;

    // Sort by topology (respecting parent-child relationships)
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .context("Failed to set revwalk sorting order.")?;

    revwalk.push(head_oid)
        .context("Failed to push HEAD OID into revwalk.")?;

    // Collect all OIDs first so we can process in chronological order
    let all_oids: Vec<git2::Oid> = revwalk.collect::<Result<Vec<_>, _>>()
        .context("Failed to iterate over commits.")?;

    // Take the newest commits first, then reverse for chronological processing.
    // revwalk returns newest-first, so .take(max_commits) gets the most recent N,
    // then .rev() puts them in chronological (oldest-first) order for analysis.
    let oids: Vec<git2::Oid> = if max_commits > 0 && max_commits < all_oids.len() {
        all_oids.into_iter().take(max_commits).rev().collect()
    } else {
        all_oids.into_iter().rev().collect()
    };

    let total = oids.len();
    let mut records = Vec::with_capacity(total);

    // Progress bar for commit extraction
    let pb = indicatif::ProgressBar::new(total as u64);
    pb.set_style(
        indicatif::ProgressStyle::with_template(
            "  {msg} [{bar:40.cyan/blue}] {pos}/{len} ({eta})"
        )
        .unwrap()
        .progress_chars("━━╸ ")
    );
    pb.set_message("◈ Extracting commits...");

    for (idx, oid) in oids.iter().enumerate() {
        pb.inc(1);

        let commit = repo.find_commit(*oid)
            .with_context(|| format!("Failed to find commit {}", oid))?;

        // Extract commit metadata
        let hash = format!("{}", oid);
        let short_hash = hash.chars().take(7).collect();
        let author = commit.author();
        let author_name = author.name().unwrap_or("<unknown>").to_string();
        let author_email = author.email().unwrap_or("<unknown>").to_string();
        let timestamp = author.when().seconds();
        let utc_offset = author.when().offset_minutes();
        let summary = commit.summary().unwrap_or("<no message>").to_string();

        // Extract file deltas (diff against parent)
        let file_deltas = extract_file_deltas(repo, &commit)?;

        // Skip merge commits with no discernible diff (rare but possible)
        // We still include them if they have file deltas
        records.push(CommitRecord {
            hash,
            short_hash,
            author_name,
            author_email,
            timestamp,
            utc_offset,
            summary,
            file_deltas,
        });

        // Progress logging
        if idx > 0 && idx % 500 == 0 {
            tracing::info!("Processed {}/{} commits", idx, total);
        }
    }

    pb.finish_with_message(format!("◈ {} commits extracted", records.len()));
    Ok(records)
}

/// Extract the list of files changed in a single commit.
///
/// For commits with a single parent, this diffs against the parent.
/// For the initial commit (no parent), this diffs against the empty tree.
/// For merge commits (multiple parents), this diffs against the first parent.
fn extract_file_deltas(repo: &Repository, commit: &git2::Commit) -> Result<Vec<FileDelta>> {
    let tree = commit.tree()
        .context("Failed to get commit tree")?;

    let parent_tree = if commit.parent_count() == 0 {
        // Initial commit: diff against empty tree
        None
    } else {
        // Diff against first parent (for merge commits, this is the "main" branch)
        let parent = commit.parent(0)
            .context("Failed to get commit parent")?;
        Some(parent.tree()
            .context("Failed to get parent tree")?)
    };

    let mut diff_opts = DiffOptions::new();
    // Configure diff options for efficiency
    diff_opts
        .skip_binary_check(true)     // Skip binary file detection for speed
        .include_unmodified(false)   // Only show changed files
        .disable_pathspec_match(true); // Don't use pathspec matching

    let diff = match &parent_tree {
        Some(pt) => repo.diff_tree_to_tree(Some(pt), Some(&tree), Some(&mut diff_opts)),
        None => repo.diff_tree_to_tree(None, Some(&tree), Some(&mut diff_opts)),
    }
    .context("Failed to compute diff")?;

    let mut deltas = Vec::new();

    for delta in diff.deltas() {
        let change_type = match delta.status() {
            git2::Delta::Added => ChangeType::Added,
            git2::Delta::Deleted => ChangeType::Deleted,
            git2::Delta::Modified => ChangeType::Modified,
            git2::Delta::Renamed => ChangeType::Renamed,
            git2::Delta::Untracked => ChangeType::Untracked,
            _ => continue, // Skip other delta types (copied, typechange, etc.)
        };

        // Use the "new" path for added/modified/renamed, "old" path for deleted
        let path = if delta.new_file().path().is_some() {
            delta.new_file().path().unwrap().to_string_lossy().to_string()
        } else if delta.old_file().path().is_some() {
            delta.old_file().path().unwrap().to_string_lossy().to_string()
        } else {
            continue; // Skip entries with no path
        };

        // Skip empty paths and binary indicator files
        if path.is_empty() {
            continue;
        }

        deltas.push(FileDelta { path, change_type });
    }

    Ok(deltas)
}

/// Format a git2::Time into a human-readable string
pub fn format_time(time: &Time) -> String {
    let secs = time.seconds();
    let offset = time.offset_minutes();
    let datetime = chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default();
    let offset_hours = offset / 60;
    let offset_mins = offset % 60;
    format!(
        "{} UTC{:+03}{:02}",
        datetime.format("%Y-%m-%d %H:%M:%S"),
        offset_hours,
        offset_mins
    )
}

/// A diff hunk representing a contiguous range of changed lines in a file.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub file_path: String,
    /// Old file start line (1-based, 0 if added)
    pub old_start: u32,
    /// Old file line count
    pub old_count: u32,
    /// New file start line (1-based, 0 if deleted)
    pub new_start: u32,
    /// New file line count
    pub new_count: u32,
}

/// Extract diff hunks for all files in a commit.
///
/// This provides line-level change information that can be mapped to
/// function-level changes using AST analysis.
pub fn extract_diff_hunks(repo: &Repository, commit: &git2::Commit) -> Result<Vec<DiffHunk>> {
    let tree = commit.tree()
        .context("Failed to get commit tree")?;

    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        let parent = commit.parent(0)
            .context("Failed to get commit parent")?;
        Some(parent.tree()
            .context("Failed to get parent tree")?)
    };

    let mut diff_opts = DiffOptions::new();
    diff_opts
        .skip_binary_check(true)
        .include_unmodified(false);

    let diff = match &parent_tree {
        Some(pt) => repo.diff_tree_to_tree(Some(pt), Some(&tree), Some(&mut diff_opts)),
        None => repo.diff_tree_to_tree(None, Some(&tree), Some(&mut diff_opts)),
    }
    .context("Failed to compute diff")?;

    let mut hunks = Vec::new();

    for delta in diff.deltas() {
        let file_path = if delta.new_file().path().is_some() {
            delta.new_file().path().unwrap().to_string_lossy().to_string()
        } else if delta.old_file().path().is_some() {
            delta.old_file().path().unwrap().to_string_lossy().to_string()
        } else {
            continue;
        };

        // Get the patch for this delta to extract hunk information
        let patch = match git2::Patch::from_diff(&diff, delta.nfiles() as usize) {
            Ok(Some(p)) => p,
            _ => continue,
        };

        let num_hunks = patch.num_hunks();
        for h in 0..num_hunks {
            if let Ok((hunk, _)) = patch.hunk(h) {
                hunks.push(DiffHunk {
                    file_path: file_path.clone(),
                    old_start: hunk.old_start(),
                    old_count: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_count: hunk.new_lines(),
                });
            }
        }
    }

    Ok(hunks)
}

/// Read the content of a file at a specific commit.
pub fn read_file_at_commit(repo: &Repository, commit: &git2::Commit, file_path: &str) -> Result<String> {
    let tree = commit.tree()
        .context("Failed to get commit tree")?;

    let entry = tree.get_path(std::path::Path::new(file_path))
        .with_context(|| format!("File {} not found in commit", file_path))?;

    let blob = repo.find_blob(entry.id())
        .with_context(|| format!("Failed to read blob for {}", file_path))?;

    let content = std::str::from_utf8(blob.content())
        .with_context(|| format!("File {} is not valid UTF-8", file_path))?;

    Ok(content.to_string())
}

/// Read the content of a file from the working tree (HEAD).
pub fn read_file_from_head(repo: &Repository, file_path: &str) -> Result<String> {
    let head = repo.head()
        .context("Failed to resolve HEAD")?;
    let commit = head.peel_to_commit()
        .context("HEAD does not point to a commit")?;
    read_file_at_commit(repo, &commit, file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_type_display() {
        assert_eq!(format!("{}", ChangeType::Added), "A");
        assert_eq!(format!("{}", ChangeType::Modified), "M");
        assert_eq!(format!("{}", ChangeType::Deleted), "D");
        assert_eq!(format!("{}", ChangeType::Renamed), "R");
    }
}
