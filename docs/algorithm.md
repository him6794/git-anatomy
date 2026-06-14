# Temporal Coupling Algorithm

## Core formula

```
Confidence(A → B) = Count(A ∩ B) / Count(A)
```

Where:
- `Count(A)` = number of commits that modify file (or function) A
- `Count(A ∩ B)` = number of commits that modify both A and B in the same commit

This is a directional measure. Confidence(A→B) and Confidence(B→A) can differ significantly. If file A is changed in 100 commits and B in 10, and they co-occur in 9 commits, then Confidence(A→B) = 9% but Confidence(B→A) = 90%. The interpretation: "90% of the time someone changes B, they also change A."

## Confidence vs. co-change count

Confidence alone is misleading. Two files that appear together in 1 out of 1 commit have 100% confidence, but that's a sample size of one.

git-anatomy uses both signals:
- **Confidence** — the ratio, used for risk classification thresholds
- **Co-change count** — the absolute number of shared commits, used for filtering and sorting

The `query_top_coupled_pairs` function applies a minimum co-change threshold (default: 3) to filter out coincidental correlations. Results are sorted by co-change count descending, so high-impact pairs appear first.

## Risk classification

The output combines two independent signals:

| Signal | Source | Meaning |
|--------|--------|---------|
| Static dependency | AST call graph | A calls B, or B calls A (at function level) |
| Temporal coupling | Git history | A and B are frequently modified in the same commit |

The intersection produces three risk levels:

| Risk | Condition | Interpretation |
|------|-----------|----------------|
| HIGH | Static dep exists AND temporal coupling >= 70% | Changes to A are very likely to require changes to B. The code dependency is active and historically verified. |
| MEDIUM | No static dep AND temporal coupling >= 70% | A and B change together despite no direct code path. This suggests a shared business requirement or indirect coupling through configuration, data schema, or convention. Worth investigating. |
| LOW | Static dep exists AND temporal coupling < 70% | A depends on B in code, but they're rarely modified together. The dependency is probably stable — refactoring is lower risk. |

The 70% threshold is a heuristic based on empirical observation. You can adjust it with `--threshold`.

## Function-level coupling

File-level coupling is coarse. Two functions in the same file might have completely different change patterns. Function-level coupling provides finer granularity.

The algorithm:

1. For each commit, extract diff hunks (line ranges that changed).
2. Parse the file at that commit to get function definitions (with line ranges).
3. Intersect diff hunks with function ranges — a function is "touched" by a commit if any diff hunk overlaps its line range.
4. Ingest (commit, file, function) triples into `function_deltas`.
5. Compute temporal coupling on function_deltas the same way as file_deltas.

Function-level coupling is queried by `check --line <N>`, which first locates the function at line N, then queries the `function_deltas` table.

## Noise filtering

Not all co-changes are meaningful. git-anatomy filters two categories of noise:

### Noise files

Changelogs, lockfiles, CI configs, and documentation files are excluded from coupling results because they change promiscuously with everything. The filter is applied:

- In `check` output — noise files are removed from results
- In `scan` top pairs — only source code files are shown
- The filter checks: exact filename matches (CHANGELOG.md, Cargo.lock, etc.), extensions (.yml, .json, .md, .rst, .toml), directory prefixes (.github/, docs/), and dotfiles at root.

### Noise calls

In the static call graph, stdlib and boilerplate calls are filtered before edge construction. Without this, the call graph drowns in noise like `Vec::new()`, `unwrap()`, `println!()`, `Some()`, `None`.

The filter is language-specific and inspects the full AST context (node kind, parent node) rather than just the extracted function name. For example, in Rust, `method_call_expression` nodes are checked against a list of known stdlib method names.

## Database caching

The coupling database is cached at `.git-anatomy/coupling.db` after `scan`. Subsequent `check` and `stats` commands load the cache instead of re-scanning.

The cache is a SQLite file written by iterating over the in-memory tables and INSERT-ing rows. This is slower than `rusqlite::backup` (which isn't available in the bundled SQLite build) but handles the common case well.

To force a re-scan, delete `.git-anatomy/coupling.db` and re-run `scan`.

## Limitations

- **Branch scope**: Only the specified branch (default: HEAD) is scanned. Coupling across branches is not computed.
- **Merge commits**: Merge commits are included, which can inflate co-change counts for files that are often merged together.
- **Rename detection**: File renames are not tracked. If `old_name.rs` is renamed to `new_name.rs`, they're treated as separate files. This can split coupling history.
- **Language support**: Function detection and call extraction depend on tree-sitter grammar quality. Some languages (notably C/C++ with macros) may produce incomplete results.
- **Cross-repo coupling**: Only one repository is analyzed at a time. Monorepo support works; multi-repo coupling does not.
