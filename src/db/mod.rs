//! Database: In-memory SQLite for high-speed commit aggregation and temporal coupling queries
//!
//! This module provides an in-memory SQLite database that stores commit history
//! and file/function deltas, then computes temporal coupling (confidence) between
//! entities using SQL aggregation.
//!
//! ## Schema
//! ```sql
//! -- Commits table: one row per commit
//! CREATE TABLE commits (
//!     id          INTEGER PRIMARY KEY,
//!     hash        TEXT NOT NULL UNIQUE,
//!     ...
//! );
//!
//! -- File deltas: one row per (commit, file) pair
//! CREATE TABLE file_deltas (
//!     id          INTEGER PRIMARY KEY,
//!     commit_id   INTEGER NOT NULL,
//!     file_path   TEXT NOT NULL,
//!     change_type TEXT NOT NULL
//! );
//!
//! -- Function deltas: one row per (commit, function) pair (Phase 2)
//! CREATE TABLE function_deltas (
//!     id             INTEGER PRIMARY KEY,
//!     commit_id      INTEGER NOT NULL,
//!     file_path      TEXT NOT NULL,
//!     function_name  TEXT NOT NULL,
//!     start_line     INTEGER,
//!     end_line       INTEGER
//! );
//! ```
//!
//! ## Temporal Coupling Algorithm
//! Confidence(A → B) = Count(A ∩ B) / Count(A)
//! where Count(A) = number of commits that touch entity A
//! and   Count(A ∩ B) = number of commits that touch both A and B

#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::git_engine::CommitRecord;

// ─── Data Types ──────────────────────────────────────────────────────────────

/// A coupled file result from temporal coupling analysis
#[derive(Debug, Clone)]
pub struct CoupledFile {
    /// The file path that is coupled to the target
    pub file_path: String,
    /// Confidence: fraction of target-file commits that also touch this file
    pub confidence: f64,
    /// Number of commits where both files appear
    pub co_commit_count: usize,
}

/// A coupled function result from function-level temporal coupling (Phase 2)
#[derive(Debug, Clone)]
pub struct CoupledFunction {
    /// Function name
    pub function_name: String,
    /// File path containing the function
    pub file_path: String,
    /// Confidence: fraction of target-function commits that also touch this function
    pub confidence: f64,
    /// Number of commits where both functions appear
    pub co_commit_count: usize,
}

/// A pair of coupled files (for the top-pairs query)
#[derive(Debug, Clone)]
pub struct CoupledPair {
    pub file_a: String,
    pub file_b: String,
    pub confidence: f64,
    pub co_commit_count: usize,
}

/// Aggregate statistics about the database
#[derive(Debug, Clone, Default)]
pub struct DatabaseStats {
    pub total_commits: usize,
    pub total_deltas: usize,
    pub unique_files: usize,
    pub unique_authors: usize,
    pub avg_files_per_commit: f64,
}

/// A file entry for listing all tracked files
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub file_path: String,
    pub commit_count: usize,
    pub change_count: usize,
}

// ─── Database Wrapper ────────────────────────────────────────────────────────

/// In-memory SQLite database for commit history and temporal coupling analysis
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Create a new in-memory SQLite database with the required schema.
    pub fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("Failed to create in-memory SQLite database")?;

        let db = Database { conn };
        db.initialize_schema()
            .context("Failed to initialize database schema")?;

        Ok(db)
    }

    /// Create a new database backed by a file (for persistence/debugging)
    pub fn open_file(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open SQLite database at {}", path.display()))?;

        let db = Database { conn };
        // Don't re-initialize schema if file already has tables
        let table_count: i64 = db.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('commits', 'file_deltas', 'function_deltas')",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        if table_count < 3 {
            db.initialize_schema()
                .context("Failed to initialize database schema")?;
        }

        Ok(db)
    }

    /// Save the in-memory database to a file on disk.
    ///
    /// This creates a new file database and copies all data into it,
    /// which can later be loaded with `open_file()`.
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        // Remove existing file if present (otherwise schema conflicts)
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("Failed to remove existing database file at {}", path.display()))?;
        }

        // Create a new file database
        let file_db = Database::open_file(path)?;
        let file_tx = file_db.conn.unchecked_transaction()
            .context("Failed to begin file database transaction")?;

        // Copy commits
        {
            let mut stmt = self.conn.prepare(
                "SELECT hash, short_hash, author_name, author_email, timestamp, utc_offset, summary FROM commits"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i32>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            for row in rows {
                let (hash, short_hash, author_name, author_email, timestamp, utc_offset, summary) = row?;
                file_tx.execute(
                    "INSERT OR IGNORE INTO commits (hash, short_hash, author_name, author_email, timestamp, utc_offset, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![hash, short_hash, author_name, author_email, timestamp, utc_offset, summary],
                )?;
            }
        }

        // Copy file_deltas
        {
            let mut stmt = self.conn.prepare(
                "SELECT fd.id, c.id, fd.file_path, fd.change_type FROM file_deltas fd JOIN commits c ON fd.commit_id = c.id"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(1)?, // commit_id (mapped)
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (commit_id, file_path, change_type) = row?;
                file_tx.execute(
                    "INSERT INTO file_deltas (commit_id, file_path, change_type) VALUES (?1, ?2, ?3)",
                    params![commit_id, file_path, change_type],
                )?;
            }
        }

        // Copy function_deltas
        {
            let mut stmt = self.conn.prepare(
                "SELECT fd.id, c.id, fd.file_path, fd.function_name, fd.start_line, fd.end_line FROM function_deltas fd JOIN commits c ON fd.commit_id = c.id"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(1)?, // commit_id (mapped)
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i32>>(4)?,
                    row.get::<_, Option<i32>>(5)?,
                ))
            })?;
            for row in rows {
                let (commit_id, file_path, function_name, start_line, end_line) = row?;
                file_tx.execute(
                    "INSERT INTO function_deltas (commit_id, file_path, function_name, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![commit_id, file_path, function_name, start_line, end_line],
                )?;
            }
        }

        file_tx.commit().context("Failed to commit file database transaction")?;

        Ok(())
    }

    /// Initialize the database schema.
    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = MEMORY;
             PRAGMA synchronous = OFF;
             PRAGMA cache_size = -64000;  -- 64MB cache

             CREATE TABLE IF NOT EXISTS commits (
                 id            INTEGER PRIMARY KEY,
                 hash          TEXT NOT NULL UNIQUE,
                 short_hash    TEXT,
                 author_name   TEXT,
                 author_email  TEXT,
                 timestamp     INTEGER,
                 utc_offset    INTEGER,
                 summary       TEXT
             );

             CREATE TABLE IF NOT EXISTS file_deltas (
                 id            INTEGER PRIMARY KEY,
                 commit_id     INTEGER NOT NULL,
                 file_path     TEXT NOT NULL,
                 change_type   TEXT NOT NULL,
                 FOREIGN KEY (commit_id) REFERENCES commits(id)
             );

             CREATE TABLE IF NOT EXISTS function_deltas (
                 id             INTEGER PRIMARY KEY,
                 commit_id      INTEGER NOT NULL,
                 file_path      TEXT NOT NULL,
                 function_name  TEXT NOT NULL,
                 start_line     INTEGER,
                 end_line       INTEGER,
                 FOREIGN KEY (commit_id) REFERENCES commits(id)
             );

             CREATE INDEX IF NOT EXISTS idx_file_deltas_path
                 ON file_deltas(file_path);

             CREATE INDEX IF NOT EXISTS idx_file_deltas_commit_id
                 ON file_deltas(commit_id);

             CREATE INDEX IF NOT EXISTS idx_file_deltas_path_commit_id
                 ON file_deltas(file_path, commit_id);

             CREATE INDEX IF NOT EXISTS idx_function_deltas_name
                 ON function_deltas(function_name);

             CREATE INDEX IF NOT EXISTS idx_function_deltas_file
                 ON function_deltas(file_path);

             CREATE INDEX IF NOT EXISTS idx_function_deltas_commit_id
                 ON function_deltas(commit_id);

             CREATE INDEX IF NOT EXISTS idx_function_deltas_file_func
                 ON function_deltas(file_path, function_name);
             "
        ).context("Failed to create database tables and indexes")?;

        Ok(())
    }

    /// Ingest a batch of commit records into the database.
    ///
    /// Uses a transaction for bulk insert performance.
    pub fn ingest_commits(&self, commits: &[CommitRecord]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()
            .context("Failed to begin database transaction")?;

        for commit in commits {
            // Insert commit record
            tx.execute(
                "INSERT OR IGNORE INTO commits (hash, short_hash, author_name, author_email, timestamp, utc_offset, summary)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    commit.hash,
                    commit.short_hash,
                    commit.author_name,
                    commit.author_email,
                    commit.timestamp,
                    commit.utc_offset,
                    commit.summary,
                ],
            ).with_context(|| format!("Failed to insert commit {}", commit.hash))?;

            // Get the auto-generated commit id
            let commit_id: i64 = tx.query_row(
                "SELECT id FROM commits WHERE hash = ?1",
                params![commit.hash],
                |row| row.get(0),
            ).with_context(|| format!("Failed to query commit id for {}", commit.hash))?;

            // Insert file deltas
            for delta in &commit.file_deltas {
                let change_type_str = format!("{}", delta.change_type);
                tx.execute(
                    "INSERT INTO file_deltas (commit_id, file_path, change_type)
                     VALUES (?1, ?2, ?3)",
                    params![commit_id, delta.path, change_type_str],
                ).with_context(|| format!("Failed to insert file delta for {} in commit {}", delta.path, commit.hash))?;
            }
        }

        tx.commit().context("Failed to commit database transaction")?;
        tracing::info!("Ingested {} commits into database", commits.len());
        Ok(())
    }

    /// Ingest function-level deltas (Phase 2).
    ///
    /// Each entry records that a specific function was modified in a commit.
    pub fn ingest_function_deltas(
        &self,
        commit_hash: &str,
        function_deltas: &[(String, String, u32, u32)], // (file_path, function_name, start_line, end_line)
    ) -> Result<()> {
        let commit_id: i64 = self.conn.query_row(
            "SELECT id FROM commits WHERE hash = ?1",
            params![commit_hash],
            |row| row.get(0),
        ).context("Commit not found in database")?;

        for (file_path, func_name, start_line, end_line) in function_deltas {
            self.conn.execute(
                "INSERT INTO function_deltas (commit_id, file_path, function_name, start_line, end_line)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![commit_id, file_path, func_name, start_line, end_line],
            )?;
        }

        Ok(())
    }

    /// Query temporal coupling for a specific file.
    pub fn query_temporal_coupling(
        &self,
        target_file: &str,
        threshold: f64,
        top_n: usize,
    ) -> Result<Vec<CoupledFile>> {
        let total_target_commits: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT fd.commit_id)
             FROM file_deltas fd
             WHERE fd.file_path = ?1",
            params![target_file],
            |row| row.get(0),
        ).context("Failed to count target file commits")?;

        if total_target_commits == 0 {
            tracing::warn!("No commits found touching file: {}", target_file);
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            "SELECT
                co_fd.file_path,
                COUNT(DISTINCT co_fd.commit_id) AS co_commit_count,
                CAST(COUNT(DISTINCT co_fd.commit_id) AS REAL) / ?1 AS confidence
             FROM file_deltas target_fd
             JOIN file_deltas co_fd
                 ON target_fd.commit_id = co_fd.commit_id
                 AND co_fd.file_path != ?2
             WHERE target_fd.file_path = ?2
             GROUP BY co_fd.file_path
             HAVING confidence >= ?3
             ORDER BY confidence DESC
             LIMIT ?4"
        ).context("Failed to prepare temporal coupling query")?;

        let results = stmt.query_map(
            params![total_target_commits, target_file, threshold, top_n as i64],
            |row| {
                Ok(CoupledFile {
                    file_path: row.get(0)?,
                    co_commit_count: row.get::<_, i64>(1)? as usize,
                    confidence: row.get(2)?,
                })
            },
        ).context("Failed to execute temporal coupling query")?;

        let coupled: Vec<CoupledFile> = results
            .filter_map(|r| r.ok())
            .collect();

        Ok(coupled)
    }

    /// Query function-level temporal coupling (Phase 2).
    ///
    /// Given a target function (file + name), find all other functions
    /// that are temporally coupled (frequently co-modified in the same commits).
    pub fn query_function_temporal_coupling(
        &self,
        target_file: &str,
        target_function: &str,
        threshold: f64,
        top_n: usize,
    ) -> Result<Vec<CoupledFunction>> {
        let total_target_commits: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT fd.commit_id)
             FROM function_deltas fd
             WHERE fd.file_path = ?1 AND fd.function_name = ?2",
            params![target_file, target_function],
            |row| row.get(0),
        ).context("Failed to count target function commits")?;

        if total_target_commits == 0 {
            // Fall back to file-level: if no function-level data, use all commits touching the file
            return self.query_temporal_coupling(target_file, threshold, top_n)
                .map(|files| files.into_iter().map(|f| CoupledFunction {
                    function_name: "(file-level)".to_string(),
                    file_path: f.file_path,
                    confidence: f.confidence,
                    co_commit_count: f.co_commit_count,
                }).collect());
        }

        let mut stmt = self.conn.prepare(
            "SELECT
                co_fd.file_path,
                co_fd.function_name,
                COUNT(DISTINCT co_fd.commit_id) AS co_commit_count,
                CAST(COUNT(DISTINCT co_fd.commit_id) AS REAL) / ?1 AS confidence
             FROM function_deltas target_fd
             JOIN function_deltas co_fd
                 ON target_fd.commit_id = co_fd.commit_id
                 AND NOT (co_fd.file_path = ?2 AND co_fd.function_name = ?3)
             WHERE target_fd.file_path = ?2 AND target_fd.function_name = ?3
             GROUP BY co_fd.file_path, co_fd.function_name
             HAVING confidence >= ?4
             ORDER BY confidence DESC
             LIMIT ?5"
        ).context("Failed to prepare function temporal coupling query")?;

        let results = stmt.query_map(
            params![total_target_commits, target_file, target_function, threshold, top_n as i64],
            |row| {
                Ok(CoupledFunction {
                    file_path: row.get(0)?,
                    function_name: row.get(1)?,
                    co_commit_count: row.get::<_, i64>(2)? as usize,
                    confidence: row.get(3)?,
                })
            },
        ).context("Failed to execute function temporal coupling query")?;

        let coupled: Vec<CoupledFunction> = results
            .filter_map(|r| r.ok())
            .collect();

        Ok(coupled)
    }

    /// Query the top N most strongly coupled file pairs in the repository.
    ///
    /// `min_co_changes` filters out pairs with too few co-change commits,
    /// preventing misleading 100% confidence for single-commit overlaps.
    pub fn query_top_coupled_pairs(
        &self,
        top_n: usize,
        threshold: f64,
        min_co_changes: usize,
    ) -> Result<Vec<CoupledPair>> {
        let mut stmt = self.conn.prepare(
            "WITH pair_counts AS (
                SELECT
                    CASE WHEN a.file_path < b.file_path THEN a.file_path ELSE b.file_path END AS file_a,
                    CASE WHEN a.file_path < b.file_path THEN b.file_path ELSE a.file_path END AS file_b,
                    COUNT(DISTINCT a.commit_id) AS co_commit_count
                FROM file_deltas a
                JOIN file_deltas b
                    ON a.commit_id = b.commit_id
                    AND a.file_path < b.file_path
                GROUP BY file_a, file_b
                HAVING co_commit_count >= ?3
            ),
            file_commit_counts AS (
                SELECT file_path, COUNT(DISTINCT commit_id) AS total_commits
                FROM file_deltas
                GROUP BY file_path
            )
            SELECT
                pc.file_a,
                pc.file_b,
                pc.co_commit_count,
                MAX(
                    CAST(pc.co_commit_count AS REAL) / fca.total_commits,
                    CAST(pc.co_commit_count AS REAL) / fcb.total_commits
                ) AS confidence
            FROM pair_counts pc
            JOIN file_commit_counts fca ON pc.file_a = fca.file_path
            JOIN file_commit_counts fcb ON pc.file_b = fcb.file_path
            WHERE confidence >= ?1
            ORDER BY confidence DESC
            LIMIT ?2"
        ).context("Failed to prepare top coupled pairs query")?;

        let results = stmt.query_map(
            params![threshold, top_n as i64, min_co_changes as i64],
            |row| {
                Ok(CoupledPair {
                    file_a: row.get(0)?,
                    file_b: row.get(1)?,
                    co_commit_count: row.get::<_, i64>(2)? as usize,
                    confidence: row.get(3)?,
                })
            },
        ).context("Failed to execute top coupled pairs query")?;

        let pairs: Vec<CoupledPair> = results
            .filter_map(|r| r.ok())
            .collect();

        Ok(pairs)
    }

    /// Get aggregate statistics about the database.
    pub fn get_stats(&self) -> Result<DatabaseStats> {
        let total_commits: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM commits",
            [],
            |row| row.get(0),
        )?;

        let total_deltas: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_deltas",
            [],
            |row| row.get(0),
        )?;

        let unique_files: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT file_path) FROM file_deltas",
            [],
            |row| row.get(0),
        )?;

        let unique_authors: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT author_name) FROM commits",
            [],
            |row| row.get(0),
        )?;

        let avg_files_per_commit = if total_commits > 0 {
            total_deltas as f64 / total_commits as f64
        } else {
            0.0
        };

        Ok(DatabaseStats {
            total_commits: total_commits as usize,
            total_deltas: total_deltas as usize,
            unique_files: unique_files as usize,
            unique_authors: unique_authors as usize,
            avg_files_per_commit,
        })
    }

    /// Find all commits that touch a given file path.
    pub fn get_commits_for_file(&self, file_path: &str) -> Result<Vec<FileCommitInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.hash, c.short_hash, c.author_name, c.timestamp, c.summary, fd.change_type
             FROM commits c
             JOIN file_deltas fd ON c.id = fd.commit_id
             WHERE fd.file_path = ?1
             ORDER BY c.timestamp DESC"
        ).context("Failed to prepare file commits query")?;

        let results = stmt.query_map(
            params![file_path],
            |row| {
                Ok(FileCommitInfo {
                    hash: row.get(0)?,
                    short_hash: row.get(1)?,
                    author_name: row.get(2)?,
                    timestamp: row.get(3)?,
                    summary: row.get(4)?,
                    change_type: row.get(5)?,
                })
            },
        )?;

        let commits: Vec<FileCommitInfo> = results.filter_map(|r| r.ok()).collect();
        Ok(commits)
    }

    /// List all tracked files with their commit counts.
    pub fn list_files(&self) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path,
                    COUNT(DISTINCT commit_id) AS commit_count,
                    COUNT(*) AS change_count
             FROM file_deltas
             GROUP BY file_path
             ORDER BY commit_count DESC"
        ).context("Failed to prepare file list query")?;

        let results = stmt.query_map([], |row| {
            Ok(FileEntry {
                file_path: row.get(0)?,
                commit_count: row.get::<_, i64>(1)? as usize,
                change_count: row.get::<_, i64>(2)? as usize,
            })
        })?;

        Ok(results.filter_map(|r| r.ok()).collect())
    }

    /// Get the top N "hottest" functions — those most frequently modified across commits.
    ///
    /// Returns function names, file paths, and commit counts from function_deltas,
    /// ordered by commit count descending.
    pub fn get_hottest_functions(&self, top_n: usize) -> Result<Vec<HotFunction>> {
        let mut stmt = self.conn.prepare(
            "SELECT function_name, file_path, COUNT(DISTINCT commit_id) AS commit_count
             FROM function_deltas
             GROUP BY function_name, file_path
             ORDER BY commit_count DESC
             LIMIT ?1"
        ).context("Failed to prepare hottest functions query")?;

        let results = stmt.query_map(
            params![top_n as i64],
            |row| {
                Ok(HotFunction {
                    function_name: row.get(0)?,
                    file_path: row.get(1)?,
                    commit_count: row.get::<_, i64>(2)? as usize,
                })
            },
        ).context("Failed to execute hottest functions query")?;

        Ok(results.filter_map(|r| r.ok()).collect())
    }
}

/// A "hottest function" — most frequently modified across commits
#[derive(Debug, Clone)]
pub struct HotFunction {
    pub function_name: String,
    pub file_path: String,
    pub commit_count: usize,
}

/// Information about a commit that touches a specific file
#[derive(Debug, Clone)]
pub struct FileCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub author_name: String,
    pub timestamp: i64,
    pub summary: String,
    pub change_type: String,
}
