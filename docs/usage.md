# Usage Guide

## Installation

```bash
# From crates.io (when published)
cargo install git-anatomy

# From source
git clone https://github.com/user/git-anatomy.git
cd git-anatomy
cargo build --release
# Binary at target/release/git-anatomy
```

### Cross-compilation for Windows (from Linux)

```bash
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

Requires Visual Studio Build Tools with C++ workload on the Windows target.

## Typical workflow

### 1. Scan the repository

```bash
cd your-project
git-anatomy scan
```

This reads the full commit history, builds the coupling database, and caches it at `.git-anatomy/coupling.db`. On a repo with ~1500 commits, this takes a few seconds.

For large repos, limit the scan:

```bash
git-anatomy scan --max-commits 500
```

### 2. Check a file before changing it

```bash
git-anatomy check --file src/parser/mod.rs
```

This shows which files are temporally coupled to `mod.rs`. If you're about to refactor that file, check the HIGH risk entries first — those are the files most likely to need updates too.

### 3. Drill into a specific function

```bash
git-anatomy check --file src/parser/mod.rs --line 42
```

This locates the function at line 42, extracts its direct calls, builds a cross-file call graph, and shows function-level coupling. The "Static?" column tells you whether a temporal coupling also has a code dependency.

### 4. Review scan results

```bash
git-anatomy scan
```

The scan output includes:
- Top temporal couplings (source files only, sorted by co-change count)
- Function scan summary (how many functions found in each file)
- Most-modified functions (candidates for refactoring or closer inspection)

### 5. Interactive exploration

```bash
git-anatomy tui
```

Keyboard-driven terminal UI for browsing coupling relationships. Useful for exploring a codebase you're not familiar with.

## Interpreting results

### Risk levels

| Risk | What it means | What to do |
|------|--------------|------------|
| HIGH | Static dependency + frequent co-changes | Test thoroughly after changes. Consider decoupling. |
| MED | No static dependency, but frequent co-changes | Investigate why they change together. There may be hidden coupling. |
| LOW | Static dependency, but rarely co-changed | The dependency is stable. Low risk to refactor. |

### Confidence thresholds

The default threshold is 0.5 (50%). Lower it to see more couplings:

```bash
git-anatomy check --file src/main.rs --threshold 0.1
```

Raise it to focus on strong couplings only:

```bash
git-anatomy check --file src/main.rs --threshold 0.8
```

### The "Static?" column

- **YES**: There's a direct code dependency (function call) between the files. The temporal coupling confirms it's active.
- **no**: No direct code dependency found. The co-changes are driven by something else — shared requirements, parallel development, or indirect coupling.

A "no" in the Static column with a high confidence is the most interesting finding. It means two files change together despite no direct code path between them. This often reveals:
- Shared configuration or data schema
- Parallel feature development that always ships together
- Implicit contracts between modules

## Tips

- **Run `scan` once, then `check` multiple times.** The database is cached, so `check` is near-instant after the first scan.
- **Use `--line` when you know which function you're changing.** File-level coupling is coarse; function-level is more actionable.
- **Pay attention to MED risk with no static dep.** That's the "invisible coupling" signal that static analysis alone can't find.
- **Delete `.git-anatomy/` to force a re-scan.** The cache doesn't auto-invalidate when new commits are added.
- **For monorepos, `--max-commits` prevents long initial scans.** You can always re-scan with a higher limit later.

## .gitignore

Consider adding `.git-anatomy/` to your `.gitignore`. The coupling database is a local cache and shouldn't be committed.

```bash
echo ".git-anatomy/" >> .gitignore
```
