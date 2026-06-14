# Architecture

git-anatomy is structured as four modules with a single-direction data flow.

```
git_engine  →  db  →  analyzer  →  (display / TUI)
   │              │         │
   │              │         └── tree-sitter grammars (bundled at compile time)
   │              └── rusqlite (in-memory, optional file cache)
   └── git2-rs (reads .git directly, no subprocess)
```

## Module responsibilities

### git_engine

Reads the raw commit DAG from `.git/objects` via `git2-rs`. Does not shell out to `git`.

Key functions:
- `open_repo(path)` — opens a git repository
- `extract_commit_history(repo, max_commits)` — walks the commit DAG, returns `Vec<CommitRecord>`
- `extract_diff_hunks(repo, commit)` — produces per-file diff hunks (old/new line ranges)
- `read_file_at_commit(repo, commit, path)` — reads a blob at a specific commit
- `read_file_from_head(repo, path)` — convenience wrapper for HEAD

`CommitRecord` carries: hash, author, timestamp, and a list of `(file_path, change_type)` pairs.

### db

In-memory SQLite database for temporal coupling computation. Uses `rusqlite` with WAL mode and `PRAGMA synchronous = OFF` for ingestion speed.

Schema:

```sql
CREATE TABLE commits (
    id          INTEGER PRIMARY KEY,
    hash        TEXT NOT NULL UNIQUE,
    author      TEXT,
    timestamp   INTEGER,
    message     TEXT
);

CREATE TABLE file_deltas (
    id          INTEGER PRIMARY KEY,
    commit_id   INTEGER NOT NULL REFERENCES commits(id),
    file_path   TEXT NOT NULL,
    change_type TEXT NOT NULL
);

CREATE TABLE function_deltas (
    id             INTEGER PRIMARY KEY,
    commit_id      INTEGER NOT NULL REFERENCES commits(id),
    file_path      TEXT NOT NULL,
    function_name  TEXT NOT NULL,
    start_line     INTEGER,
    end_line       INTEGER
);
```

Key queries:
- `query_temporal_coupling(file, threshold, top_n)` — computes confidence for all files co-occurring with the target
- `query_function_temporal_coupling(file, func, threshold, top_n)` — same but at function granularity
- `query_top_coupled_pairs(limit, min_confidence, min_co_changes)` — global top-N coupled pairs
- `get_hottest_functions(n)` — functions with the most distinct commits touching them

Persistence: after `scan`, the in-memory DB is copied to `.git-anatomy/coupling.db` via manual row-by-row SQL INSERT. The `check` and `stats` commands try to load this cache first.

### analyzer

AST-based analysis using tree-sitter. Each supported language has a grammar compiled into the binary at build time.

Key functions:
- `detect_language(path)` — maps file extension to `Language` enum
- `extract_functions(path, source)` — returns `Vec<FunctionDef>` with name, start_line, end_line
- `extract_calls_from_function(source, func)` — collects call names from the AST subtree
- `map_diff_to_functions(path, source, diff_ranges)` — given changed line ranges, returns which functions overlap
- `build_call_graph(file_sources)` — builds `Vec<CallEdge>` from caller/callee pairs across files
- `classify_risk(has_static, confidence)` — maps (static dep, temporal coupling) to `RiskLevel`

Noise filtering: stdlib calls (e.g., `Vec::new`, `println!`, `unwrap`) are filtered before call graph construction. The filter is language-specific and inspects AST context (not just the extracted name).

### tui

Terminal UI built on `ratatui`. Keyboard-driven, with panels for file list, coupling detail, and function view. Requires a TTY — falls back gracefully in non-interactive environments.

## Data flow: `scan` command

1. Open repo → extract all commits (with progress bar)
2. Ingest commits into SQLite (file_deltas populated from git diff)
3. For each commit: extract diff hunks → map to functions → ingest function_deltas
4. Save DB to `.git-anatomy/coupling.db`
5. Print summary: stats, top coupled pairs, function scan, hot functions

## Data flow: `check` command

1. Load cached DB (or rebuild from scratch if no cache)
2. Query temporal coupling for target file
3. Filter noise files
4. Read target + coupled file sources from HEAD
5. Build static call graph
6. Display file-level results with risk classification
7. If `--line` specified: locate function, extract calls, query function coupling, display combined results
