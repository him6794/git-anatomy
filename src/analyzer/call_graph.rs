//! Call graph builder: static analysis of function call relationships
//!
//! Walks through source files, extracts function definitions and
//! their call sites, then builds a directed graph of call edges.
//! Only edges with resolved (in-project) callees are included.

use anyhow::Result;
use std::collections::HashMap;

use super::{CallEdge, extract_functions, extract_calls_from_function, detect_language};

/// Strip qualifiers from a call name to get the short form for resolution.
/// "bound::without_defaults" → "without_defaults"
/// "params.split_for_impl"   → "split_for_impl"
fn short_name(name: &str) -> String {
    if let Some(pos) = name.rfind("::") {
        let s = &name[pos + 2..];
        s.split('<').next().unwrap_or(s).to_string()
    } else if let Some(pos) = name.rfind('.') {
        name[pos + 1..].to_string()
    } else {
        name.to_string()
    }
}

/// Build a static call graph from a list of source files.
///
/// Returns a list of directed edges: caller → callee.
/// Only includes edges where the callee is found in the project
/// (callee_file is Some). External/stdlib calls are excluded.
pub fn build_call_graph(
    files: &HashMap<String, String>,
) -> Result<Vec<CallEdge>> {
    let mut edges = Vec::new();
    // Primary index: simple function name → (file, language)
    let mut function_index: HashMap<String, (String, String)> = HashMap::new();

    // Phase 1: Index all function definitions by their simple name
    for (file_path, source) in files {
        if let Some(language) = detect_language(file_path) {
            if let Ok(functions) = extract_functions(file_path, source) {
                for func in functions {
                    function_index.entry(func.name.clone())
                        .or_insert_with(|| (file_path.clone(), language.clone()));
                }
            }
        }
    }

    // Phase 2: Extract call edges from each file
    for (file_path, source) in files {
        if detect_language(file_path).is_none() {
            continue;
        }

        if let Ok(functions) = extract_functions(file_path, source) {
            for func in &functions {
                if let Ok(calls) = extract_calls_from_function(source, func) {
                    for callee_name in calls {
                        // Try exact match first, then short name (strip qualifiers)
                        let callee_file = function_index
                            .get(&callee_name)
                            .or_else(|| function_index.get(&short_name(&callee_name)))
                            .map(|(f, _)| f.clone());

                        // Only add edges for resolved in-project callees
                        if let Some(callee_file) = callee_file {
                            // Use the resolved short name as callee_name for consistent matching
                            let resolved_name = if function_index.contains_key(&callee_name) {
                                callee_name
                            } else {
                                short_name(&callee_name)
                            };

                            edges.push(CallEdge {
                                caller_name: func.name.clone(),
                                caller_file: file_path.clone(),
                                callee_name: resolved_name,
                                callee_file: Some(callee_file),
                            });
                        }
                        // Skip unresolved (external/stdlib) calls entirely
                    }
                }
            }
        }
    }

    Ok(edges)
}

/// Find all functions that directly call a target function.
#[allow(dead_code)]
pub fn find_callers_of(
    target_func: &str,
    edges: &[CallEdge],
) -> Vec<CallEdge> {
    edges.iter()
        .filter(|e| e.callee_name == target_func)
        .cloned()
        .collect()
}

/// Find all functions called by a target function.
#[allow(dead_code)]
pub fn find_callees_of(
    target_func: &str,
    target_file: &str,
    edges: &[CallEdge],
) -> Vec<CallEdge> {
    edges.iter()
        .filter(|e| e.caller_name == target_func && e.caller_file == target_file)
        .cloned()
        .collect()
}

/// Find all functions in the same file that have a static dependency on
/// the target function (either calling it or being called by it).
#[allow(dead_code)]
pub fn find_static_dependencies(
    target_func: &str,
    target_file: &str,
    edges: &[CallEdge],
) -> Vec<String> {
    let mut deps = std::collections::HashSet::new();

    for edge in edges {
        // Direct callers of our target
        if edge.callee_name == target_func {
            deps.insert(format!("{}::{}", edge.caller_file, edge.caller_name));
        }
        // Direct callees from our target
        if edge.caller_name == target_func && edge.caller_file == target_file {
            if let Some(ref callee_file) = edge.callee_file {
                deps.insert(format!("{}::{}", callee_file, edge.callee_name));
            }
        }
    }

    deps.into_iter().collect()
}
