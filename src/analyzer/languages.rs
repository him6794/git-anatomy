//! Language detection and tree-sitter grammar configuration
//!
//! Supports: Rust, JavaScript, TypeScript, Python, Go, Java, C/C++
//! Each language defines which AST node types represent:
//! - Function/method definitions
//! - Call expressions
//! - How to extract function names and call targets

use tree_sitter::Language;

// ─── Language Detection ──────────────────────────────────────────────────────

/// Detect the programming language from a file path's extension.
pub fn detect_language_from_path(file_path: &str) -> Option<String> {
    let ext = file_path.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        "rs" => Some("rust".to_string()),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript".to_string()),
        "ts" => Some("typescript".to_string()),
        "tsx" => Some("tsx".to_string()),
        "py" | "pyw" | "pyi" => Some("python".to_string()),
        "go" => Some("go".to_string()),
        "java" => Some("java".to_string()),
        "c" | "h" => Some("c".to_string()),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp".to_string()),
        _ => None,
    }
}

/// Get the tree-sitter Language for a given language name.
pub fn get_language(name: &str) -> Option<Language> {
    match name {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        _ => None,
    }
}

// ─── Function Node Classification ────────────────────────────────────────────

/// Check if an AST node kind represents a function or method definition.
pub fn is_function_node(kind: &str, language: &str) -> bool {
    let function_kinds = function_node_kinds(language);
    function_kinds.contains(&kind)
}

/// Get the set of AST node kinds that represent function/method definitions
/// for a given language.
fn function_node_kinds(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &[
            "function_item",
            "function_signature_item",
            "impl_item",
        ],
        "javascript" | "typescript" | "tsx" => &[
            "function_declaration",
            "function_expression",
            "method_definition",
            "arrow_function",
            "generator_function_declaration",
            "generator_function",
            "class_method",
        ],
        "python" => &[
            "function_definition",
            "decorated_definition",
        ],
        "go" => &[
            "function_declaration",
            "method_declaration",
        ],
        "java" => &[
            "method_declaration",
            "constructor_declaration",
            "class_declaration",
            "interface_declaration",
        ],
        "c" | "cpp" => &[
            "function_definition",
            "declaration",
        ],
        _ => &[],
    }
}

// ─── Function Name Extraction ────────────────────────────────────────────────

/// Given a function node, find the child node that contains the function name.
pub fn get_function_name_node<'a>(node: &tree_sitter::Node<'a>, language: &str) -> Option<tree_sitter::Node<'a>> {
    match language {
        "rust" => {
            node.child_by_field_name("name")
        }
        "javascript" | "typescript" | "tsx" => {
            // First try the standard name field (works for function_declaration, method_definition)
            if let Some(name_node) = node.child_by_field_name("name") {
                return Some(name_node);
            }

            // For anonymous function expressions, look at the parent context
            // to derive a name from the variable or property it's assigned to
            let parent = node.parent();
            match parent.as_ref().map(|p| p.kind()) {
                Some("variable_declarator") => {
                    // var foo = function() { ... }
                    if let Some(parent_node) = parent {
                        parent_node.child_by_field_name("name")
                    } else {
                        None
                    }
                }
                Some("assignment_expression") => {
                    // obj.foo = function() { ... }
                    if let Some(parent_node) = parent {
                        if let Some(left) = parent_node.child_by_field_name("left") {
                            if left.kind() == "member_expression" {
                                left.child_by_field_name("property")
                            } else {
                                Some(left)
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Some("property") => {
                    // { foo: function() { ... } }
                    if let Some(parent_node) = parent {
                        parent_node.child_by_field_name("key")
                    } else {
                        None
                    }
                }
                Some("pair") => {
                    // { foo: function() { ... } } (in some grammars)
                    if let Some(parent_node) = parent {
                        parent_node.child_by_field_name("key")
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        "python" => {
            if node.kind() == "decorated_definition" {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "function_definition" {
                        return child.child_by_field_name("name");
                    }
                }
                None
            } else {
                node.child_by_field_name("name")
            }
        }
        "go" => {
            node.child_by_field_name("name")
        }
        "java" => {
            node.child_by_field_name("name")
        }
        "c" | "cpp" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "function_declarator" || child.kind() == "declarator" {
                    let mut inner_cursor = child.walk();
                    for inner_child in child.children(&mut inner_cursor) {
                        if inner_child.kind() == "identifier" {
                            return Some(inner_child);
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// ─── Call Expression Classification ──────────────────────────────────────────

/// Check if an AST node kind represents a function/method call.
pub fn is_call_node(kind: &str, language: &str) -> bool {
    let call_kinds = call_node_kinds(language);
    call_kinds.contains(&kind)
}

/// Get the set of AST node kinds that represent function/method calls.
fn call_node_kinds(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &[
            "call_expression",
            "macro_invocation",
            "method_call_expression",
        ],
        "javascript" | "typescript" | "tsx" => &[
            "call_expression",
            "new_expression",
        ],
        "python" => &[
            "call",
        ],
        "go" => &[
            "call_expression",
        ],
        "java" => &[
            "method_invocation",
            "object_creation_expression",
            "constructor_invocation",
        ],
        "c" | "cpp" => &[
            "call_expression",
        ],
        _ => &[],
    }
}

/// Extract the name of the function being called from a call expression node.
pub fn extract_call_name(node: &tree_sitter::Node, source: &str, language: &str) -> Option<String> {
    match language {
        "rust" => extract_rust_call_name(node, source),
        "javascript" | "typescript" | "tsx" => extract_js_call_name(node, source),
        "python" => extract_python_call_name(node, source),
        "go" => extract_go_call_name(node, source),
        "java" => extract_java_call_name(node, source),
        "c" | "cpp" => extract_c_call_name(node, source),
        _ => None,
    }
}

fn extract_rust_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    match node.kind() {
        "call_expression" => {
            let func = node.child(0)?;
            match func.kind() {
                "scoped_identifier" | "generic_function" => {
                    // For qualified calls like bound::without_defaults or Vec::<T>::new,
                    // extract just the final identifier after ::
                    let text = &source[func.byte_range()];
                    let short = text.rsplit("::").next().unwrap_or(text);
                    // Strip generic parameters like <T> from the short name
                    let short = short.split('<').next().unwrap_or(short);
                    Some(short.to_string())
                }
                "field_expression" => {
                    // For field calls like obj.method, extract just the method name
                    // (the last identifier after '.')
                    func.child_by_field_name("field")
                        .map(|n| source[n.byte_range()].to_string())
                        .or_else(|| {
                            let text = &source[func.byte_range()];
                            let short = text.rsplit('.').next().unwrap_or(text);
                            Some(short.to_string())
                        })
                }
                _ => {
                    let text = &source[func.byte_range()];
                    Some(text.to_string())
                }
            }
        }
        "method_call_expression" => {
            // Return object.method for resolution context;
            // the caller will normalize to just the method name when needed
            let method = node.child_by_field_name("method")
                .map(|n| source[n.byte_range()].to_string());
            let object = node.child_by_field_name("object")
                .map(|n| source[n.byte_range()].to_string());

            match (object, method) {
                (Some(obj), Some(meth)) => Some(format!("{}.{}", obj, meth)),
                (None, Some(meth)) => Some(meth),
                _ => {
                    let func = node.child(1)?;
                    Some(source[func.byte_range()].to_string())
                }
            }
        }
        "macro_invocation" => {
            let func = node.child(0)?;
            Some(source[func.byte_range()].to_string())
        }
        _ => None,
    }
}

fn extract_js_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    match node.kind() {
        "call_expression" => {
            let func = node.child(0)?;
            let text = &source[func.byte_range()];
            if func.kind() == "member_expression" {
                let property = func.child_by_field_name("property");
                property
                    .map(|p| source[p.byte_range()].to_string())
                    .or_else(|| Some(text.to_string()))
            } else {
                Some(text.to_string())
            }
        }
        "new_expression" => {
            let constructor = node.child_by_field_name("constructor")?;
            Some(format!("new {}", &source[constructor.byte_range()]))
        }
        _ => None,
    }
}

fn extract_python_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let func = node.child(0)?;
    let text = &source[func.byte_range()];
    if func.kind() == "attribute" {
        let attr = func.child_by_field_name("attribute");
        attr.map(|a| source[a.byte_range()].to_string())
            .or_else(|| Some(text.to_string()))
    } else {
        Some(text.to_string())
    }
}

fn extract_go_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let func = node.child(0)?;
    let text = &source[func.byte_range()];
    if func.kind() == "selector_expression" {
        let field = func.child_by_field_name("field");
        field
            .map(|f| source[f.byte_range()].to_string())
            .or_else(|| Some(text.to_string()))
    } else {
        Some(text.to_string())
    }
}

fn extract_java_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    match node.kind() {
        "method_invocation" => {
            node.child_by_field_name("name")
                .map(|n| source[n.byte_range()].to_string())
        }
        "object_creation_expression" => {
            let constructor = node.child_by_field_name("type")?;
            Some(format!("new {}", &source[constructor.byte_range()]))
        }
        _ => None,
    }
}

fn extract_c_call_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let func = node.child_by_field_name("function");
    func.map(|f| source[f.byte_range()].to_string())
        .or_else(|| {
            let first = node.child(0)?;
            Some(source[first.byte_range()].to_string())
        })
}
