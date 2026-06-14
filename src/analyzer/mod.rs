//! Analyzer: AST-based function-level analysis (Phase 2)
//!
//! This module uses tree-sitter to:
//! 1. Parse source files into ASTs for multiple languages
//! 2. Locate the function/method at a given line number
//! 3. Extract all function definitions from a file
//! 4. Build static call chains between functions
//! 5. Map diff hunks to function-level changes
//! 6. Classify risk by combining static dependency with temporal coupling

mod languages;
mod call_graph;

pub use call_graph::build_call_graph;

use anyhow::{Context, Result};
use std::collections::HashMap;
use tree_sitter::{Node, Parser, Tree};

// ─── Data Types ──────────────────────────────────────────────────────────────

/// Represents a function or method identified in source code
#[derive(Debug, Clone)]
pub struct FunctionDef {
    /// Function/method name
    pub name: String,
    /// File path containing the function
    pub file_path: String,
    /// Start line (1-based)
    pub start_line: u32,
    /// End line (1-based, inclusive)
    pub end_line: u32,
    /// Language of the source file
    pub language: String,
}

impl FunctionDef {
    /// Check if a given line (1-based) falls within this function's range
    pub fn contains_line(&self, line: u32) -> bool {
        line >= self.start_line && line <= self.end_line
    }
}

/// Represents a call relationship between two functions
#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller_name: String,
    pub caller_file: String,
    pub callee_name: String,
    /// None if callee is external (not defined in the analyzed codebase)
    pub callee_file: Option<String>,
}

/// Risk level classification combining static dependency and temporal coupling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// 🔴 High: Strong code dependency AND highly synchronized changes
    High,
    /// 🟠 Medium: No direct code dependency but Git history shows high synchronization (hidden business coupling)
    Medium,
    /// 🟡 Low: Code dependency exists but rarely synchronized changes
    Low,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::High => write!(f, "HIGH"),
            RiskLevel::Medium => write!(f, "MEDIUM"),
            RiskLevel::Low => write!(f, "LOW"),
        }
    }
}

impl RiskLevel {
    pub fn icon(&self) -> &str {
        match self {
            RiskLevel::High => "🔴",
            RiskLevel::Medium => "🟠",
            RiskLevel::Low => "🟡",
        }
    }
}

/// A function-level coupling result combining static analysis and temporal coupling
#[derive(Debug, Clone)]
pub struct FunctionCoupling {
    pub function_name: String,
    pub file_path: String,
    pub confidence: f64,
    pub has_static_call: bool,
    pub risk_level: RiskLevel,
}

/// Classify the risk level based on static dependency and temporal coupling.
///
/// - 🔴 High: has_static_call=true AND temporal_coupling >= 0.7
/// - 🟠 Medium: has_static_call=false AND temporal_coupling >= 0.7
/// - 🟡 Low: everything else with some coupling signal
pub fn classify_risk(has_static_call: bool, temporal_coupling: f64) -> RiskLevel {
    if has_static_call && temporal_coupling >= 0.7 {
        RiskLevel::High
    } else if !has_static_call && temporal_coupling >= 0.7 {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    }
}

// ─── Core AST Operations ─────────────────────────────────────────────────────

/// Parse a source file using tree-sitter with the appropriate language grammar.
fn parse_source(source: &str, language_name: &str) -> Result<Option<Tree>> {
    let language = match languages::get_language(language_name) {
        Some(lang) => lang,
        None => {
            tracing::warn!("Unsupported language: {}, skipping AST parse", language_name);
            return Ok(None);
        }
    };

    let mut parser = Parser::new();
    parser.set_language(&language)
        .context("Failed to set tree-sitter language")?;

    let tree = parser.parse(source, None)
        .context("Failed to parse source code")?;

    Ok(Some(tree))
}

/// Detect the programming language from a file extension.
pub fn detect_language(file_path: &str) -> Option<String> {
    languages::detect_language_from_path(file_path)
}

/// Find the function definition at a given line in a source file.
///
/// Uses tree-sitter to parse the file and find the function/method node
/// that spans the given line number.
pub fn find_function_at_line(file_path: &str, source: &str, line: u32) -> Result<Option<FunctionDef>> {
    let language_name = match detect_language(file_path) {
        Some(lang) => lang,
        None => return Ok(None),
    };

    let tree = match parse_source(source, &language_name)? {
        Some(t) => t,
        None => return Ok(None),
    };

    let root = tree.root_node();

    // Walk the AST looking for function nodes that contain the given line
    if let Some(func_node) = find_function_node(&root, line, &language_name) {
        let name = extract_function_name(&func_node, source, &language_name);
        Ok(Some(FunctionDef {
            name,
            file_path: file_path.to_string(),
            start_line: (func_node.start_position().row + 1) as u32,
            end_line: (func_node.end_position().row + 1) as u32,
            language: language_name,
        }))
    } else {
        Ok(None)
    }
}

/// Extract all function definitions from a source file.
pub fn extract_functions(file_path: &str, source: &str) -> Result<Vec<FunctionDef>> {
    let language_name = match detect_language(file_path) {
        Some(lang) => lang,
        None => return Ok(Vec::new()),
    };

    let tree = match parse_source(source, &language_name)? {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };

    let root = tree.root_node();
    let mut functions = Vec::new();

    collect_function_nodes(&root, source, file_path, &language_name, &mut functions);

    Ok(functions)
}

/// Extract function names called from within a given function body.
pub fn extract_calls_from_function(
    source: &str,
    func: &FunctionDef,
) -> Result<Vec<String>> {
    let language_name = match detect_language(&func.file_path) {
        Some(lang) => lang,
        None => return Ok(Vec::new()),
    };

    let tree = match parse_source(source, &language_name)? {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };

    let root = tree.root_node();

    // Find the function node
    if let Some(func_node) = find_function_node(&root, func.start_line, &language_name) {
        let calls = collect_call_names(&func_node, source, &language_name);
        Ok(calls)
    } else {
        Ok(Vec::new())
    }
}

/// Map diff line ranges to the functions that were modified.
///
/// Given a list of (start_line, end_line) ranges from a diff,
/// returns the set of functions that overlap with those ranges.
pub fn map_diff_to_functions(
    file_path: &str,
    source: &str,
    diff_ranges: &[(u32, u32)],
) -> Result<Vec<FunctionDef>> {
    let functions = extract_functions(file_path, source)?;

    let mut modified_functions = Vec::new();

    for func in &functions {
        for &(start, end) in diff_ranges {
            // Check if the diff range overlaps with the function range
            if start <= func.end_line && end >= func.start_line {
                modified_functions.push(func.clone());
                break;
            }
        }
    }

    Ok(modified_functions)
}

// ─── AST Traversal Helpers ───────────────────────────────────────────────────

/// Recursively search for the innermost function node that contains the given line.
fn find_function_node<'a>(node: &Node<'a>, line: u32, language: &str) -> Option<Node<'a>> {
    let target_row = line as usize - 1; // Convert 1-based to 0-based

    // Check if this node contains the target line
    if node.start_position().row > target_row || node.end_position().row < target_row {
        return None;
    }

    // Check if this node IS a function-like construct
    let is_function = languages::is_function_node(node.kind(), language);

    // Recurse into children first (to find the most specific/innermost function)
    let mut child_result = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_function_node(&child, line, language) {
            child_result = Some(found);
            break;
        }
    }

    // If a child function was found, prefer it (innermost match)
    child_result.or_else(|| {
        if is_function {
            Some(*node)
        } else {
            None
        }
    })
}

/// Collect all function definition nodes in the tree.
fn collect_function_nodes<'a>(
    node: &Node<'a>,
    source: &str,
    file_path: &str,
    language: &str,
    functions: &mut Vec<FunctionDef>,
) {
    let is_function = languages::is_function_node(node.kind(), language);

    if is_function {
        let name = extract_function_name(node, source, language);
        functions.push(FunctionDef {
            name,
            file_path: file_path.to_string(),
            start_line: (node.start_position().row + 1) as u32,
            end_line: (node.end_position().row + 1) as u32,
            language: language.to_string(),
        });
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_function_nodes(&child, source, file_path, language, functions);
    }
}

/// Extract the name of a function from its AST node.
fn extract_function_name(node: &Node, source: &str, language: &str) -> String {
    let name_node = languages::get_function_name_node(node, language);

    match name_node {
        Some(name_node) => {
            let text = &source[name_node.byte_range()];
            // For namespaced names (e.g., "Struct::method"), return the full qualified name
            text.to_string()
        }
        None => {
            // Fallback: try to derive a name from context
            let text = &source[node.byte_range()];
            let first_line = text.lines().next().unwrap_or("");
            let trimmed = first_line.trim();

            // For anonymous functions, don't use the generic "function" text
            if trimmed.starts_with("function") || trimmed.starts_with("=>") || trimmed.starts_with("async") {
                // Try to find a contextual name from the parent node
                if let Some(parent) = node.parent() {
                    match parent.kind() {
                        "variable_declarator" => {
                            if let Some(name_n) = parent.child_by_field_name("name") {
                                return source[name_n.byte_range()].to_string();
                            }
                        }
                        "assignment_expression" => {
                            if let Some(left) = parent.child_by_field_name("left") {
                                if left.kind() == "member_expression" {
                                    if let Some(prop) = left.child_by_field_name("property") {
                                        return source[prop.byte_range()].to_string();
                                    }
                                }
                                return source[left.byte_range()].to_string();
                            }
                        }
                        "property" | "pair" => {
                            if let Some(key) = parent.child_by_field_name("key") {
                                return source[key.byte_range()].to_string();
                            }
                        }
                        _ => {}
                    }
                }

                // Last resort: use "anonymous" instead of "function"
                return "anonymous".to_string();
            }

            trimmed.to_string()
        }
    }
}

// ─── Noise Filtering ─────────────────────────────────────────────────────────

/// Known stdlib/boilerplate qualified call names to filter out.
static KNOWN_NOISE: &[&str] = &[
    // Collection constructors
    "Vec::new", "HashSet::new", "HashMap::new", "Box::new",
    // Option / Result constructors
    "Option::Some", "Option::None", "Result::Ok", "Result::Err",
    // Common trait methods (qualified form)
    "String::from", "Into::into", "From::from",
    "Clone::clone", "ToOwned::to_owned", "ToString::to_string",
    "Default::default",
    // Iterator methods (qualified form)
    "Iterator::collect", "Iterator::map", "Iterator::filter", "Iterator::next",
    // Option / Result methods (qualified form)
    "Option::unwrap", "Option::unwrap_or", "Option::unwrap_or_else",
    "Option::is_some", "Option::is_none",
    "Result::unwrap", "Result::unwrap_err", "Result::is_ok", "Result::is_err",
];

/// Prefixes that indicate a stdlib qualified call (noise).
static NOISE_PREFIXES: &[&str] = &[
    "Iterator::", "Option::", "Result::",
    "Vec::", "HashSet::", "HashMap::", "Box::",
    "String::", "Clone::", "Default::",
    "Into::", "From::", "ToOwned::", "ToString::",
    "AsRef::", "AsMut::", "Borrow::", "BorrowMut::",
    "Deref::", "DerefMut::",
];

/// Simple (unqualified) function/macro names that are always noise.
static SIMPLE_NOISE: &[&str] = &[
    "panic", "println", "eprintln", "format", "vec",
    "assert", "assert_eq", "assert_ne",
    "todo", "unimplemented", "unreachable",
    "cfg", "env", "include_str", "include_bytes",
    // Variant constructors (usually written without qualifier)
    "Some", "None", "Ok", "Err",
    // Common bare constructor
    "new", "default",
];

/// Short method names from stdlib traits that are almost always noise
/// when appearing as method calls (e.g., `.collect()`, `.unwrap()`).
static SHORT_NOISE_METHODS: &[&str] = &[
    // Iterator
    "collect", "map", "filter", "filter_map", "flat_map", "next",
    "for_each", "fold", "find", "any", "all", "count",
    "into_iter", "iter", "iter_mut", "enumerate", "zip", "chain",
    "skip", "take", "rev", "peekable", "inspect",
    // Option / Result
    "unwrap", "unwrap_or", "unwrap_or_else", "unwrap_or_default",
    "unwrap_err", "unwrap_none",
    "is_some", "is_none", "is_ok", "is_err", "expect",
    "ok", "err", "or", "and", "or_else", "and_then",
    "map_ok", "map_err", "map_or", "map_or_else",
    // Clone / Default / Into / AsRef etc.
    "clone", "to_owned", "to_string", "default", "into",
    "as_ref", "as_mut", "borrow", "borrow_mut",
    "deref", "deref_mut",
    // Collection methods (Vec, HashSet, HashMap, etc.)
    "push", "pop", "insert", "remove", "contains", "len", "is_empty",
    "get", "get_mut", "entry", "or_insert", "extend", "with_capacity",
    "from_iter", "from_elem", "last", "first", "split_off",
    // Common constructors
    "new", "from", "from_iter",
];

/// Macro names that are always noise.
static MACRO_NOISE: &[&str] = &[
    "println", "eprintln", "format", "vec",
    "assert", "assert_eq", "assert_ne",
    "panic",
    "todo", "unimplemented", "unreachable",
    "cfg", "env", "include_str", "include_bytes",
];

/// Check whether a Rust call AST node represents a noise (stdlib/boilerplate) call.
/// Uses the full AST context for accurate filtering, not just the extracted short name.
fn is_rust_noise_call(node: &Node, source: &str) -> bool {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child(0) {
                let text = &source[func.byte_range()];

                // Exact match on the full qualified name (e.g. "HashSet::new")
                if KNOWN_NOISE.contains(&text) {
                    return true;
                }

                // Prefix match (e.g. anything starting with "Iterator::")
                for prefix in NOISE_PREFIXES {
                    if text.starts_with(prefix) {
                        return true;
                    }
                }

                // For field expressions ("obj.method"), check the last segment
                if text.contains('.') {
                    let short = text.rsplit('.').next().unwrap_or(text);
                    if SHORT_NOISE_METHODS.contains(&short) {
                        return true;
                    }
                }

                // For unqualified names (no ::), check against SIMPLE_NOISE.
                // Qualified names (e.g. "Parameters::new") are NOT checked here
                // because the qualifier provides context that disambiguates them
                // from stdlib calls like "Vec::new".
                if !text.contains("::") {
                    let check = text.split('<').next().unwrap_or(text);
                    if SIMPLE_NOISE.contains(&check) {
                        return true;
                    }
                }
            }
            false
        }
        "method_call_expression" => {
            // For method calls, check the method name against SHORT_NOISE_METHODS
            if let Some(method_node) = node.child_by_field_name("method") {
                let method = &source[method_node.byte_range()];
                if SHORT_NOISE_METHODS.contains(&method) {
                    return true;
                }
            }
            false
        }
        "macro_invocation" => {
            if let Some(macro_node) = node.child(0) {
                let macro_name = &source[macro_node.byte_range()];
                // Strip trailing '!' for comparison
                let name = macro_name.strip_suffix('!').unwrap_or(macro_name);
                if MACRO_NOISE.contains(&name) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Normalize a call name to its short form (strip qualifiers).
/// "bound::without_defaults" → "without_defaults"
/// "params.split_for_impl" → "split_for_impl"
fn normalize_call_name(name: &str) -> String {
    // Strip qualifiers: take the last segment after :: or .
    if let Some(pos) = name.rfind("::") {
        let short = &name[pos + 2..];
        // Also strip generic parameters
        short.split('<').next().unwrap_or(short).to_string()
    } else if let Some(pos) = name.rfind('.') {
        name[pos + 1..].to_string()
    } else {
        name.to_string()
    }
}

/// Collect all call expression names within a function node,
/// filtering out noise (stdlib/boilerplate) and normalizing to short names.
fn collect_call_names(func_node: &Node, source: &str, language: &str) -> Vec<String> {
    let mut calls = Vec::new();
    collect_call_names_recursive(func_node, source, language, &mut calls);
    calls
}

fn collect_call_names_recursive<'a>(
    node: &Node<'a>,
    source: &str,
    language: &str,
    calls: &mut Vec<String>,
) {
    // Check if this is a call expression
    if languages::is_call_node(node.kind(), language) {
        // Filter noise calls using the full AST context
        let is_noise = match language {
            "rust" => is_rust_noise_call(node, source),
            _ => false, // No noise filtering for other languages yet
        };

        if !is_noise {
            if let Some(name) = languages::extract_call_name(node, source, language) {
                // Normalize to short name for resolution
                let normalized = normalize_call_name(&name);
                if !normalized.is_empty() {
                    calls.push(normalized);
                }
            }
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_call_names_recursive(&child, source, language, calls);
    }
}

/// Analyze a specific function and compute function-level coupling.
///
/// This is the main entry point for Phase 2 function-level analysis.
pub fn analyze_function_coupling(
    target_file: &str,
    target_line: u32,
    source: &str,
    all_files: &HashMap<String, String>,
    call_edges: &[CallEdge],
    file_coupling: &[(String, f64, usize)],
) -> Result<Vec<FunctionCoupling>> {
    // Step 1: Find the target function
    let target_func = find_function_at_line(target_file, source, target_line)?;

    let target_func = match target_func {
        Some(f) => f,
        None => {
            tracing::warn!("No function found at line {} in {}", target_line, target_file);
            return Ok(Vec::new());
        }
    };

    // Step 2: Extract calls from the target function
    let direct_calls = extract_calls_from_function(source, &target_func)?;

    // Step 3: Build a set of files that contain the directly called functions
    let mut static_dep_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    for edge in call_edges {
        if edge.caller_name == target_func.name && edge.caller_file == target_func.file_path {
            if let Some(ref callee_file) = edge.callee_file {
                static_dep_files.insert(callee_file.clone());
            }
        }
        // Also check reverse: something calls our target function
        if edge.callee_name == target_func.name {
            static_dep_files.insert(edge.caller_file.clone());
        }
    }

    // Step 4: For files that are also called directly, add to static deps
    for call_name in &direct_calls {
        for (file_path, file_source) in all_files {
            if let Ok(funcs) = extract_functions(file_path, file_source) {
                for func in funcs {
                    if func.name == *call_name {
                        static_dep_files.insert(file_path.clone());
                    }
                }
            }
        }
    }

    // Step 5: Combine temporal coupling with static analysis
    let mut results = Vec::new();

    for (coupled_file, confidence, _co_commits) in file_coupling {
        let has_static = static_dep_files.contains(coupled_file);
        let risk = classify_risk(has_static, *confidence);

        // Try to find the specific function in the coupled file
        if let Some(file_source) = all_files.get(coupled_file) {
            if let Ok(funcs) = extract_functions(coupled_file, file_source) {
                for func in funcs {
                    // Check if any function in the coupled file is directly called
                    let func_has_static = direct_calls.contains(&func.name);
                    let func_risk = classify_risk(func_has_static, *confidence);

                    results.push(FunctionCoupling {
                        function_name: func.name,
                        file_path: coupled_file.clone(),
                        confidence: *confidence,
                        has_static_call: func_has_static,
                        risk_level: func_risk,
                    });
                }
            }
        }

        // Also add a file-level entry
        results.push(FunctionCoupling {
            function_name: "(file-level)".to_string(),
            file_path: coupled_file.clone(),
            confidence: *confidence,
            has_static_call: has_static,
            risk_level: risk,
        });
    }

    // Sort by risk level (High first), then by confidence
    results.sort_by(|a, b| {
        let risk_order = |r: &RiskLevel| match r {
            RiskLevel::High => 0,
            RiskLevel::Medium => 1,
            RiskLevel::Low => 2,
        };
        risk_order(&a.risk_level).cmp(&risk_order(&b.risk_level))
            .then_with(|| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
    });

    Ok(results)
}
