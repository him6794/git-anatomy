//! git-anatomy: Find out what else breaks when you change a function
//!
//! Cross-references static code dependencies (AST) with historical co-change
//! patterns (Git temporal coupling) to flag coupled modules.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod analyzer;
mod db;
mod git_engine;
mod tui;

// ─── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "git-anatomy",
    version,
    about = "Find out what else breaks when you change a function",
    long_about = "git-anatomy cross-references static code dependencies (AST) with \
                  historical co-change patterns (Git) to flag coupled modules.",
    arg_required_else_help = true
)]
struct Cli {
    /// Path to the git repository (defaults to current directory)
    #[arg(long, short, global = true, default_value = ".")]
    repo: PathBuf,

    /// Verbosity level (-v=info, -vv=debug, -vvv=trace)
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Analyze what's coupled to a file or function
    Check {
        /// Target file path (relative to repo root)
        #[arg(long, short)]
        file: String,

        /// Line number within the file to locate the target function
        #[arg(long, short)]
        line: Option<u32>,

        /// Confidence threshold for temporal coupling (0.0 - 1.0)
        #[arg(long, short, default_value = "0.5")]
        threshold: f64,

        /// Maximum number of coupled entities to display
        #[arg(long, short, default_value = "10")]
        top: usize,

        /// Analysis granularity: "file" or "function"
        #[arg(long, short, default_value = "file")]
        granularity: String,

        /// Maximum number of commits to process (0 = all)
        #[arg(long, default_value = "0")]
        max_commits: usize,
    },

    /// Build coupling database from Git history
    Scan {
        /// Maximum number of commits to process (0 = all)
        #[arg(long, default_value = "0")]
        max_commits: usize,

        /// Branch to scan (defaults to HEAD)
        #[arg(long, short, default_value = "HEAD")]
        branch: String,
    },

    /// Interactive terminal UI
    Tui,

    /// Show repository coupling statistics
    Stats,
}

// ─── Source file / noise detection ────────────────────────────────────────────

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "js", "ts", "tsx", "py", "go", "java", "c", "h",
    "cpp", "hpp", "cc", "cxx", "jsx", "mjs", "cjs", "pyw", "pyi",
];

fn is_source_file(file_path: &str) -> bool {
    let ext = file_path.rsplit('.').next().unwrap_or("").to_lowercase();
    SOURCE_EXTENSIONS.contains(&ext.as_str())
}

const NOISE_FILENAMES: &[&str] = &[
    "CHANGES.rst", "CHANGES.md", "CHANGES.txt", "CHANGES",
    "CHANGELOG.rst", "CHANGELOG.md", "CHANGELOG.txt", "CHANGELOG",
    "HISTORY.rst", "HISTORY.md", "HISTORY.txt", "HISTORY",
    "NEWS.rst", "NEWS.md", "NEWS.txt", "NEWS",
    "AUTHORS", "CONTRIBUTORS",
    "LICENSE", "LICENSE.txt", "LICENSE.rst", "LICENSE.md",
    "COPYING", "NOTICE",
    "package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
    "Cargo.toml", "Cargo.lock", "go.mod", "go.sum",
    "Gemfile", "Gemfile.lock",
    "requirements.txt", "setup.py", "setup.cfg", "pyproject.toml",
];

fn is_noise_file(file_path: &str) -> bool {
    let basename = file_path.rsplit('/').next().unwrap_or(file_path);
    let basename_lower = basename.to_lowercase();

    if NOISE_FILENAMES.iter().any(|n| basename_lower == n.to_lowercase()) {
        return true;
    }

    let noise_extensions: &[&str] = &[
        "yml", "yaml",
        "toml", "ini", "cfg",
        "rst", "md", "txt",
        "json", "lock",
        "xml",
    ];
    let ext = basename.rsplit('.').next().unwrap_or("").to_lowercase();
    if noise_extensions.contains(&ext.as_str()) {
        return true;
    }

    if file_path.starts_with(".github/")
        || file_path.starts_with(".gitlab/")
        || file_path.starts_with("docs/")
        || file_path.starts_with("doc/")
        || file_path.starts_with(".ci/")
        || file_path.starts_with("api/next/")
        || file_path.starts_with("doc/next/")
        || file_path.starts_with("examples/")
    {
        return true;
    }

    if basename.starts_with('.') && !file_path.contains('/') {
        return true;
    }

    false
}

// ─── Entry point ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Check { file, line, threshold, top, granularity, max_commits } => {
            check_command(&cli.repo, &file, line, threshold, top, &granularity, max_commits)?
        }
        Commands::Scan { max_commits, branch } => {
            scan_command(&cli.repo, max_commits, &branch)?
        }
        Commands::Tui => {
            tui_command(&cli.repo)?
        }
        Commands::Stats => {
            stats_command(&cli.repo)?
        }
    }

    Ok(())
}

// ─── check ────────────────────────────────────────────────────────────────────

fn check_command(
    repo_path: &Path,
    file: &str,
    line: Option<u32>,
    threshold: f64,
    top: usize,
    granularity: &str,
    max_commits: usize,
) -> Result<()> {
    print_header(&format!("Analyzing: {}", file));
    if let Some(ln) = line {
        println!("  line {}", ln);
    }
    println!("  threshold={}, top={}, granularity={}", threshold, top, granularity);
    println!();

    // Open repo
    let repo = git_engine::open_repo(repo_path)
        .context("Failed to open repository. Is this a git repo?")?;

    // Try cached DB
    let db_dir = repo_path.join(".git-anatomy");
    let db_path = db_dir.join("coupling.db");
    let mut needs_rebuild = false;

    let database = if db_path.exists() {
        println!("Loading cached database...");
        match db::Database::open_file(&db_path) {
            Ok(db) => {
                let stats = db.get_stats().unwrap_or_default();
                println!("  {} commits, {} file deltas", stats.total_commits, stats.total_deltas);
                db
            }
            Err(_) => {
                needs_rebuild = true;
                db::Database::new().context("Failed to create database.")?
            }
        }
    } else {
        needs_rebuild = true;
        db::Database::new().context("Failed to create database.")?
    };

    let database = if needs_rebuild {
        println!(
            "Scanning commit history {}",
            if max_commits > 0 { format!("(max {})", max_commits) } else { "(all)".to_string() }
        );
        let commits = git_engine::extract_commit_history(&repo, max_commits)
            .context("Failed to extract commit history.")?;
        println!("  {} commits processed", commits.len());

        println!("Building coupling database...");
        let db = db::Database::new().context("Failed to create database.")?;
        db.ingest_commits(&commits)
            .context("Failed to ingest commits.")?;

        let func_delta_count = populate_function_deltas(&repo, &commits, &db)
            .context("Failed to populate function deltas.")?;
        if func_delta_count > 0 {
            println!("  {} function deltas ingested", func_delta_count);
        }

        if let Err(e) = std::fs::create_dir_all(&db_dir) {
            tracing::warn!("Failed to create .git-anatomy directory: {}", e);
        } else {
            match db.save_to_file(&db_path) {
                Ok(_) => println!("  Cached to .git-anatomy/coupling.db"),
                Err(e) => tracing::warn!("Failed to cache database: {}", e),
            }
        }

        db
    } else {
        database
    };

    // Query temporal coupling
    println!("Computing temporal coupling for {}...", file);
    println!();

    let coupled_raw = database.query_temporal_coupling(file, threshold, top)
        .context("Failed to compute temporal coupling.")?;

    let coupled: Vec<db::CoupledFile> = coupled_raw.into_iter()
        .filter(|c| !is_noise_file(&c.file_path))
        .collect();

    if coupled.is_empty() {
        println!("  No files with coupling >= {:.0}% for '{}'", threshold * 100.0, file);
        return Ok(());
    }

    // Build call edges for static dependency detection
    let mut file_sources: HashMap<String, String> = HashMap::new();
    if let Ok(source) = git_engine::read_file_from_head(&repo, file) {
        file_sources.insert(file.to_string(), source);
    }
    for coupled_file in &coupled {
        if let Ok(coupled_source) = git_engine::read_file_from_head(&repo, &coupled_file.file_path) {
            file_sources.insert(coupled_file.file_path.clone(), coupled_source);
        }
    }
    let display_call_edges = analyzer::build_call_graph(&file_sources).unwrap_or_default();

    display_coupling_results(file, &coupled, threshold, &display_call_edges);

    // Function-level analysis
    if let Some(target_line) = line {
        println!();
        println!("{}", "Function-level analysis".bold());

        match git_engine::read_file_from_head(&repo, file) {
            Ok(source) => {
                match analyzer::find_function_at_line(file, &source, target_line) {
                    Ok(Some(func)) => {
                        println!("  Function: {} (lines {}-{})", func.name.bold(), func.start_line, func.end_line);

                        let all_functions = analyzer::extract_functions(file, &source)
                            .unwrap_or_default();
                        println!("  {} functions found in {}", all_functions.len(), file);

                        let direct_calls = analyzer::extract_calls_from_function(&source, &func)
                            .unwrap_or_default();
                        if !direct_calls.is_empty() {
                            println!("  Direct calls: {}", direct_calls.join(", "));
                        }

                        // Build static call graph across coupled files
                        let mut file_sources: HashMap<String, String> = HashMap::new();
                        file_sources.insert(file.to_string(), source.clone());
                        for coupled_file in &coupled {
                            if let Ok(coupled_source) = git_engine::read_file_from_head(&repo, &coupled_file.file_path) {
                                file_sources.insert(coupled_file.file_path.clone(), coupled_source);
                            }
                        }

                        let call_edges = analyzer::build_call_graph(&file_sources)
                            .unwrap_or_default();
                        println!("  {} static call edges", call_edges.len());

                        let func_coupled = database.query_function_temporal_coupling(
                            file, &func.name, threshold, top
                        ).unwrap_or_default();

                        display_function_coupling_results(
                            &func.name,
                            &direct_calls,
                            &call_edges,
                            &func_coupled,
                            &coupled,
                        );
                    }
                    Ok(None) => {
                        println!("  No function at line {} in {}", target_line, file);
                        if let Some(lang) = analyzer::detect_language(file) {
                            println!("  Language: {}", lang);
                        } else {
                            println!("  Unsupported file type for AST analysis");
                        }
                    }
                    Err(e) => {
                        println!("  AST parse error: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("  Could not read file from HEAD: {}", e);
            }
        }
    }

    Ok(())
}

// ─── scan ─────────────────────────────────────────────────────────────────────

fn scan_command(repo_path: &Path, max_commits: usize, branch: &str) -> Result<()> {
    print_header(&format!("Scanning: {}", repo_path.display()));
    println!("  branch={}, max_commits={}", branch, if max_commits == 0 { "all".to_string() } else { max_commits.to_string() });
    println!();

    let repo = git_engine::open_repo(repo_path)
        .context("Failed to open repository.")?;

    let commits = git_engine::extract_commit_history(&repo, max_commits)
        .context("Failed to extract commit history.")?;
    println!("  {} commits extracted", commits.len());

    let database = db::Database::new()
        .context("Failed to create database.")?;
    database.ingest_commits(&commits)
        .context("Failed to ingest commits.")?;

    let func_delta_count = populate_function_deltas(&repo, &commits, &database)
        .context("Failed to populate function deltas.")?;

    // Cache DB
    let db_dir = repo_path.join(".git-anatomy");
    let db_path = db_dir.join("coupling.db");
    match database.save_to_file(&db_path) {
        Ok(_) => println!("  Cached to .git-anatomy/coupling.db"),
        Err(e) => tracing::warn!("Failed to cache database: {}", e),
    }

    // Summary
    let stats = database.get_stats()
        .context("Failed to retrieve statistics.")?;

    println!();
    print_header("Summary");
    println!("  Commits:       {}", stats.total_commits);
    println!("  File deltas:   {}", stats.total_deltas);
    println!("  Unique files:  {}", stats.unique_files);
    println!("  Unique authors: {}", stats.unique_authors);

    // Top coupled pairs (source files only)
    println!();
    println!("{}", "Top temporal couplings (source files)".bold());

    let top_pairs = database.query_top_coupled_pairs(2000, 0.1, 2)
        .context("Failed to query top coupled pairs.")?;

    let mut source_pairs: Vec<db::CoupledPair> = top_pairs.iter()
        .filter(|p| is_source_file(&p.file_a) && is_source_file(&p.file_b))
        .filter(|p| !is_noise_file(&p.file_a) && !is_noise_file(&p.file_b))
        .cloned()
        .collect();

    source_pairs.sort_by_key(|b| std::cmp::Reverse(b.co_commit_count));

    let display_pairs: Vec<db::CoupledPair> = if source_pairs.is_empty() {
        println!("  No source-only pairs found; showing all.");
        let mut fallback: Vec<db::CoupledPair> = top_pairs.into_iter()
            .filter(|p| !is_noise_file(&p.file_a) && !is_noise_file(&p.file_b))
            .collect();
        fallback.sort_by_key(|b| std::cmp::Reverse(b.co_commit_count));
        fallback.into_iter().take(10).collect()
    } else {
        source_pairs.into_iter().take(10).collect()
    };

    if display_pairs.is_empty() {
        println!("  No significant couplings found.");
    } else {
        for (i, pair) in display_pairs.iter().enumerate() {
            let confidence_pct = pair.confidence * 100.0;
            let confidence_display = format_confidence(confidence_pct);
            println!(
                "  {:>2}. {} <-> {}  [{} confidence, {} co-changes]",
                i + 1,
                pair.file_a,
                pair.file_b,
                confidence_display,
                pair.co_commit_count,
            );
        }
    }

    // Function scan summary
    println!();
    println!("{}", "Function scan".bold());

    let files = database.list_files()?;
    let mut total_functions = 0;
    let mut files_parsed = 0;

    for file_entry in files.iter().take(50) {
        if let Some(_lang) = analyzer::detect_language(&file_entry.file_path) {
            if let Ok(source) = git_engine::read_file_from_head(&repo, &file_entry.file_path) {
                if let Ok(functions) = analyzer::extract_functions(&file_entry.file_path, &source) {
                    let func_count = functions.len();
                    if func_count > 0 {
                        total_functions += func_count;
                        files_parsed += 1;
                        println!("  {} -- {} functions", file_entry.file_path, func_count);
                    }
                }
            }
        }
    }

    println!();
    println!("  {} functions across {} files with AST support", total_functions, files_parsed);

    if func_delta_count > 0 {
        println!("  {} function deltas from commit history", func_delta_count);
    }

    // Hot functions
    if func_delta_count > 0 {
        println!();
        println!("{}", "Most-modified functions".bold());

        let hot_funcs = database.get_hottest_functions(10)
            .context("Failed to query hottest functions.")?;

        if hot_funcs.is_empty() {
            println!("  No function-level data available.");
        } else {
            println!(
                "  {:<4} {:<40} {:<30} Commits",
                "#", "Function", "File"
            );
            println!("  {}", "-".repeat(85));

            for (i, hf) in hot_funcs.iter().enumerate() {
                let marker = if hf.commit_count >= 20 {
                    "**"
                } else if hf.commit_count >= 10 {
                    "*"
                } else {
                    " "
                };
                println!(
                    "  {:<4} {} {:<37} {:<30} {}",
                    format!("{}", i + 1),
                    marker,
                    hf.function_name,
                    truncate_path(&hf.file_path, 30),
                    hf.commit_count,
                );
            }
            println!("  (** >= 20 commits, * >= 10 commits)");
        }
    }

    Ok(())
}

fn tui_command(repo_path: &Path) -> Result<()> {
    tui::run_tui(repo_path)
}

fn stats_command(repo_path: &Path) -> Result<()> {
    print_header(&format!("Statistics: {}", repo_path.display()));

    let db_path = repo_path.join(".git-anatomy/coupling.db");
    let database = if db_path.exists() {
        match db::Database::open_file(&db_path) {
            Ok(db) => {
                println!("  Using cached database");
                db
            }
            Err(_) => {
                println!("  Rebuilding database (cache invalid)...");
                let repo = git_engine::open_repo(repo_path)?;
                let commits = git_engine::extract_commit_history(&repo, 0)?;
                let db = db::Database::new()?;
                db.ingest_commits(&commits)?;
                db
            }
        }
    } else {
        println!("  No cached database found, scanning from scratch.");
        println!("  Tip: run 'git-anatomy scan' first for faster queries.");
        let repo = git_engine::open_repo(repo_path)?;
        let commits = git_engine::extract_commit_history(&repo, 0)?;
        let db = db::Database::new()?;
        db.ingest_commits(&commits)?;
        db
    };

    let stats = database.get_stats()?;

    println!();
    println!("  Commits:          {}", stats.total_commits);
    println!("  File deltas:      {}", stats.total_deltas);
    println!("  Unique files:     {}", stats.unique_files);
    println!("  Unique authors:   {}", stats.unique_authors);
    println!("  Avg files/commit: {:.2}", stats.avg_files_per_commit);

    Ok(())
}

// ─── Display ──────────────────────────────────────────────────────────────────

fn print_header(text: &str) {
    println!("{}", text.bold());
}

fn display_coupling_results(
    target_file: &str,
    results: &[db::CoupledFile],
    threshold: f64,
    call_edges: &[analyzer::CallEdge],
) {
    println!("  Coupled to {}", target_file.bold());
    println!();
    println!(
        "  {:<4} {:<50} {:>12} {:>10} {:>8} Risk",
        "#", "File", "Confidence", "Co-changes", "Static?"
    );
    println!("  {}", "-".repeat(100));

    for (i, result) in results.iter().enumerate() {
        let confidence_pct = result.confidence * 100.0;
        let (_risk_label, risk_display) = classify_display(confidence_pct);

        let confidence_display = format_confidence(confidence_pct);

        let has_static = call_edges.iter().any(|e| {
            (e.caller_file == target_file && e.callee_file.as_deref() == Some(&result.file_path))
            || (e.callee_file.as_deref() == Some(target_file) && e.caller_file == result.file_path)
        });

        let static_label = if has_static { "YES".red().bold() } else { "no".normal() };

        println!(
            "  {:<4} {:<50} {:>12} {:>10} {:>8} {}",
            format!("{}", i + 1),
            result.file_path,
            confidence_display,
            result.co_commit_count,
            static_label,
            risk_display,
        );
    }

    println!();
    let noise_count = results.iter().filter(|r| is_noise_file(&r.file_path)).count();
    if noise_count > 0 {
        println!("  {} noise file(s) filtered (changelogs, lockfiles, etc.)", noise_count);
    }
    println!("  Threshold: {:.0}% | Confidence(A->B) = commits(A&B) / commits(A)", threshold * 100.0);
}

fn display_function_coupling_results(
    target_func: &str,
    direct_calls: &[String],
    call_edges: &[analyzer::CallEdge],
    func_coupled: &[db::CoupledFunction],
    file_coupled: &[db::CoupledFile],
) {
    println!();
    println!("  Combined risk for {}", target_func.bold());
    println!();

    let static_callees: std::collections::HashSet<String> = direct_calls.iter().cloned().collect();
    let static_callers: std::collections::HashSet<String> = call_edges.iter()
        .filter(|e| e.callee_name == target_func)
        .map(|e| e.caller_name.clone())
        .collect();

    let func_coupled_files: std::collections::HashSet<String> = func_coupled.iter()
        .map(|f| f.file_path.clone())
        .collect();

    let func_coupled_is_file_fallback = func_coupled.iter()
        .all(|f| f.function_name == "(file-level)");

    println!(
        "  {:<4} {:<30} {:<30} {:>10} {:>8} {:>8} Risk",
        "#", "Entity", "File", "Confid.", "Static", "Co-chg"
    );
    println!("  {}", "-".repeat(120));

    let mut row_idx = 0;

    if func_coupled_is_file_fallback {
        for coupled in file_coupled {
            let has_static = call_edges.iter().any(|e| {
                (e.caller_name == target_func && e.callee_file.as_deref() == Some(&coupled.file_path))
                || (e.callee_name == target_func && e.caller_file == coupled.file_path)
            });

            let risk = analyzer::classify_risk(has_static, coupled.confidence);
            let confidence_pct = coupled.confidence * 100.0;
            let confidence_display = format_confidence(confidence_pct);

            row_idx += 1;
            println!(
                "  {:<4} {:<30} {:<30} {:>10} {:>8} {:>8} {}",
                format!("{}", row_idx),
                "(file-level)",
                truncate_path(&coupled.file_path, 30),
                confidence_display,
                if has_static { "YES".red() } else { "no".normal() },
                coupled.co_commit_count,
                format_risk(risk),
            );
        }
    } else {
        for coupled in func_coupled {
            let has_static = static_callees.contains(&coupled.function_name)
                || static_callers.contains(&coupled.function_name);
            let risk = analyzer::classify_risk(has_static, coupled.confidence);

            let confidence_pct = coupled.confidence * 100.0;
            let confidence_display = format_confidence(confidence_pct);

            row_idx += 1;
            println!(
                "  {:<4} {:<30} {:<30} {:>10} {:>8} {:>8} {}",
                format!("{}", row_idx),
                coupled.function_name,
                truncate_path(&coupled.file_path, 30),
                confidence_display,
                if has_static { "YES".red() } else { "no".normal() },
                coupled.co_commit_count,
                format_risk(risk),
            );
        }

        for coupled in file_coupled {
            if func_coupled_files.contains(&coupled.file_path) {
                continue;
            }

            let has_static = call_edges.iter().any(|e| {
                (e.caller_name == target_func && e.callee_file.as_deref() == Some(&coupled.file_path))
                || (e.callee_name == target_func && e.caller_file == coupled.file_path)
            });

            let risk = analyzer::classify_risk(has_static, coupled.confidence);

            let confidence_pct = coupled.confidence * 100.0;
            let confidence_display = format_confidence(confidence_pct);

            row_idx += 1;
            println!(
                "  {:<4} {:<30} {:<30} {:>10} {:>8} {:>8} {}",
                format!("{}", row_idx),
                "(file-level)",
                truncate_path(&coupled.file_path, 30),
                confidence_display,
                if has_static { "YES".red() } else { "no".normal() },
                coupled.co_commit_count,
                format_risk(risk),
            );
        }
    }

    println!();
    println!("  HIGH = static dep + temporal coupling | MEDIUM = hidden coupling (no dep, but sync changes) | LOW = dep but rare sync");
}

fn format_confidence(confidence_pct: f64) -> colored::ColoredString {
    if confidence_pct >= 70.0 {
        format!("{:.1}%", confidence_pct).red().bold()
    } else if confidence_pct >= 50.0 {
        format!("{:.1}%", confidence_pct).yellow()
    } else {
        format!("{:.1}%", confidence_pct).normal()
    }
}

fn classify_display(confidence_pct: f64) -> (&'static str, colored::ColoredString) {
    if confidence_pct >= 70.0 {
        ("HIGH", "HIGH".red().bold())
    } else if confidence_pct >= 50.0 {
        ("MEDIUM", "MED".yellow())
    } else {
        ("LOW", "LOW".normal())
    }
}

fn format_risk(risk: analyzer::RiskLevel) -> String {
    match risk {
        analyzer::RiskLevel::High => "HIGH".to_string(),
        analyzer::RiskLevel::Medium => "MED".to_string(),
        analyzer::RiskLevel::Low => "LOW".to_string(),
    }
}

fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        path.to_string()
    } else {
        let start = path.len() - max_len + 3;
        format!("...{}", &path[start..])
    }
}

// ─── Function delta population ────────────────────────────────────────────────

fn populate_function_deltas(
    repo: &git2::Repository,
    commits: &[git_engine::CommitRecord],
    database: &db::Database,
) -> Result<usize> {
    let pb = ProgressBar::new(commits.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {msg} [{bar:40.cyan/blue}] {pos}/{len} ({eta})"
        )
        .unwrap()
        .progress_chars("=- ")
    );
    pb.set_message("Mapping functions...");

    let mut total_ingested = 0usize;

    for commit_record in commits {
        pb.inc(1);

        let oid = match git2::Oid::from_str(&commit_record.hash) {
            Ok(oid) => oid,
            Err(_) => continue,
        };
        let commit_obj = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let hunks = match git_engine::extract_diff_hunks(repo, &commit_obj) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if hunks.is_empty() {
            continue;
        }

        let mut hunks_by_file: HashMap<&str, Vec<&git_engine::DiffHunk>> = HashMap::new();
        for hunk in &hunks {
            hunks_by_file.entry(&hunk.file_path).or_default().push(hunk);
        }

        let mut all_func_deltas: Vec<(String, String, u32, u32)> = Vec::new();

        for (file_path, file_hunks) in &hunks_by_file {
            if analyzer::detect_language(file_path).is_none() {
                continue;
            }

            let source = match git_engine::read_file_at_commit(repo, &commit_obj, file_path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let diff_ranges: Vec<(u32, u32)> = file_hunks.iter()
                .filter_map(|hunk| {
                    if hunk.new_count > 0 {
                        Some((hunk.new_start, hunk.new_start + hunk.new_count.saturating_sub(1)))
                    } else {
                        None
                    }
                })
                .collect();

            if diff_ranges.is_empty() {
                continue;
            }

            match analyzer::map_diff_to_functions(file_path, &source, &diff_ranges) {
                Ok(functions) => {
                    for func in functions {
                        all_func_deltas.push((
                            file_path.to_string(),
                            func.name,
                            func.start_line,
                            func.end_line,
                        ));
                    }
                }
                Err(_) => continue,
            }
        }

        if !all_func_deltas.is_empty() {
            total_ingested += all_func_deltas.len();
            if let Err(e) = database.ingest_function_deltas(&commit_record.hash, &all_func_deltas) {
                tracing::debug!("Failed to ingest function deltas for {}: {}", commit_record.hash, e);
            }
        }
    }

    pb.finish_with_message(format!("Mapped {} function deltas", total_ingested));

    Ok(total_ingested)
}

// ─── Utilities ────────────────────────────────────────────────────────────────

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_target(false)
        .init();
}
