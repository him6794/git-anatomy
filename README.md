# git-anatomy

Find out what else breaks when you change a function.

git-anatomy cross-references two signals — static code dependencies (from AST) and historical co-change patterns (from Git) — to flag which modules are coupled to your change target, even when there's no direct import or call.

[![crates.io](https://img.shields.io/crates/v/git-anatomy.svg)](https://crates.io/crates/v/git-anatomy)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![build](https://github.com/user/git-anatomy/actions/workflows/ci.yml/badge.svg)]()

---

## Why

Static analysis tells you what calls what. Git history tells you what actually changes together. Neither signal alone is enough:

- Two files with no import relationship might change together in 80% of commits — that's a hidden business coupling your linter won't catch.
- Two files that import each other might almost never change in the same commit — that dependency is probably safe to refactor.

git-anatomy intersects both signals so you can tell the difference.

## What it does

- **Temporal coupling**: Counts how often two files (or functions) appear in the same commit, then computes a confidence score: `P(B|A) = commits(A∧B) / commits(A)`.
- **Static call graph**: Parses source with tree-sitter, extracts function definitions and call edges.
- **Risk classification**: Combines both signals into three tiers:
  - **HIGH** — static dependency + high temporal coupling. Changes are likely to propagate.
  - **MEDIUM** — no static dependency, but high temporal coupling. Hidden coupling; worth investigating.
  - **LOW** — static dependency exists, but low temporal coupling. The dependency is probably stable.

## Quick Start

```bash
cargo install git-anatomy
```

Or build from source:

```bash
git clone https://github.com/user/git-anatomy.git
cd git-anatomy
cargo build --release
```

### Commands

```bash
# Analyze what's coupled to a file
git-anatomy check --file src/parser/mod.rs

# Drill into a specific function (line number to locate it)
git-anatomy check --file src/parser/mod.rs --line 42

# Full repository scan — builds a coupling database
git-anatomy scan

# Repository statistics
git-anatomy stats

# Interactive terminal UI
git-anatomy tui
```

## Example

Running `git-anatomy check` on serde's `serde_derive/src/bound.rs`:

```
$ git-anatomy check --file serde_derive/src/bound.rs --line 100

Analyzing: serde_derive/src/bound.rs (line 100)

  #  File                          Confidence  Co-changes  Static?  Risk
  ──────────────────────────────────────────────────────────────────────────
  1  serde_derive/src/attr.rs          61.5%        16        YES    HIGH
  2  serde_derive/src/de.rs            50.0%        13        YES    HIGH
  3  serde_derive/src/ser.rs           34.6%         9        YES    MEDIUM

  Function: without_defaults (lines 100-112)
  Direct calls: attr::maybe_type_attr, attributes
  5 static call edges discovered

  #  Entity                   File                         Confidence  Static  Co-chg  Risk
  ────────────────────────────────────────────────────────────────────────────────────────
  1  without_defaults          serde_derive/src/bound.rs       100.0%    no       4    MEDIUM
  2  expand_derive_serialize   serde_derive/src/ser.rs          34.6%    YES      9    MEDIUM
```

The key insight: `attr.rs` and `de.rs` show both a code dependency (Static? = YES) and a history of co-changes (50%+ confidence). That's HIGH risk. If you touch `bound.rs`, expect to update those files too.

## How it works

```
git2-rs ──► SQLite (in-memory) ──► Risk Classifier
(commits)    (aggregation)         (AST ∩ Git history)
                 ▲                         ▲
          diff hunks                  tree-sitter
          → function deltas           (AST + call graph)
```

1. **Git engine** reads `.git` directly (no `git` subprocess), walks the commit DAG, extracts file deltas and diff hunks.
2. **Database** loads commit data into an in-memory SQLite instance with optimized PRAGMAs for fast aggregation.
3. **Analyzer** parses source files with tree-sitter, maps diff hunks to function definitions, and builds a static call graph.
4. **Risk engine** intersects the static call graph with temporal coupling to produce the final classification.

Coupling database is cached at `.git-anatomy/coupling.db` after `scan`, so subsequent `check` and `stats` runs are near-instant.

## Supported languages

| Language | Extensions | Function detection | Call extraction |
|----------|-----------|:-:|:-:|
| Rust | .rs | yes | yes |
| JavaScript | .js, .jsx, .mjs | yes | yes |
| TypeScript | .ts, .tsx | yes | yes |
| Python | .py, .pyw, .pyi | yes | yes |
| Go | .go | yes | yes |
| Java | .java | yes | yes |
| C/C++ | .c, .h, .cpp, .hpp | yes | yes |

## CLI reference

```
git-anatomy [OPTIONS] <COMMAND>

Commands:
  check    Analyze what's coupled to a file or function
  scan     Build coupling database from Git history
  tui      Interactive terminal UI
  stats    Show repository coupling statistics

Options:
  -r, --repo <PATH>     Repository path [default: .]
  -v, --verbose         Verbosity (-v, -vv, -vvv)
  -h, --help
  -V, --version

check:
  -f, --file <FILE>         Target file
  -l, --line <LINE>         Line number (locates target function)
  -t, --threshold <FLOAT>   Confidence threshold 0.0–1.0 [default: 0.5]
      --top <N>             Max coupled entities [default: 10]
  -g, --granularity <G>     "file" or "function" [default: file]

scan:
      --max-commits <N>     Limit commits processed (0 = all) [default: 0]
  -b, --branch <BRANCH>     Branch [default: HEAD]
```

## Algorithm

```
Confidence(A → B) = Count(A ∩ B) / Count(A)
```

If file A was touched in 100 commits, and file B was touched in the same commit 70 of those times, Confidence(A→B) = 70%. Meaning: 70% of the time someone changes A, they also change B.

The top-coupled-pairs query applies a minimum co-change threshold (default: 3) to filter out coincidental single-commit correlations.

## TUI keybindings

| Key | Action |
|-----|--------|
| `j`/↓ | Down |
| `k`/↑ | Up |
| `Enter`/→ | Select |
| `←`/`h` | Back |
| `Tab` | Next panel |
| `/` | Search |
| `c` | Toggle coupling view (temporal / static / combined) |
| `f` | Toggle function view |
| `q`/`Esc` | Quit |

## License

MIT
