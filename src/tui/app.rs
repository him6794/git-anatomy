//! TUI Application State
//!
//! Manages the state of the interactive TUI, including:
//! - File list and selection
//! - Coupling data and display mode
//! - Current view (file browser, coupling map, function view)
//! - Search state

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::analyzer;
use crate::db;
use crate::git_engine;

// ─── View States ─────────────────────────────────────────────────────────────

/// Which panel is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    FileList,
    CouplingMap,
    Details,
}

/// Which coupling view mode is active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouplingView {
    Temporal,
    Static,
    Combined,
}

/// Which detail level is shown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLevel {
    File,
    Function,
}

// ─── App State ───────────────────────────────────────────────────────────────

/// The main application state for the TUI
pub struct App {
    /// Database connection
    pub database: db::Database,
    /// Repository path
    pub repo_path: PathBuf,

    // ─── File List ───────────────────────────────────────────────
    /// All tracked files
    pub files: Vec<db::FileEntry>,
    /// Currently selected file index
    pub file_list_state: ListState,
    /// Search query (empty = no search)
    pub search_query: String,
    /// Whether we're currently typing a search
    pub searching: bool,
    /// Filtered files based on search
    pub filtered_files: Vec<usize>,

    // ─── Coupling Data ───────────────────────────────────────────
    /// Coupling results for the currently selected file
    pub coupled_files: Vec<db::CoupledFile>,
    /// Coupling list selection
    pub coupling_list_state: ListState,
    /// Top coupled pairs (for the scan view)
    pub top_pairs: Vec<db::CoupledPair>,

    // ─── Function Data (Phase 2) ────────────────────────────────
    /// Functions found in the currently selected file
    pub functions: Vec<analyzer::FunctionDef>,
    /// Function list selection
    pub function_list_state: ListState,
    /// Function-level coupling results
    pub func_couplings: Vec<db::CoupledFunction>,
    /// Call edges from the selected file
    pub call_edges: Vec<analyzer::CallEdge>,

    // ─── UI State ────────────────────────────────────────────────
    /// Currently focused panel
    pub active_panel: Panel,
    /// Current coupling view mode
    pub coupling_view: CouplingView,
    /// Current detail level
    pub detail_level: DetailLevel,
    /// Status message
    pub status_message: String,
    /// Should the app quit?
    pub quit: bool,
}

/// Simple list selection state (avoids ratatui ListState complexity)
#[derive(Debug, Clone)]
pub struct ListState {
    pub selected: Option<usize>,
    #[allow(dead_code)]
    pub offset: usize,
}

impl ListState {
    pub fn new() -> Self {
        ListState {
            selected: None,
            offset: 0,
        }
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index;
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
}

impl App {
    /// Create a new App instance with the given database and repo path.
    pub fn new(database: db::Database, repo_path: PathBuf) -> Self {
        App {
            database,
            repo_path,
            files: Vec::new(),
            file_list_state: ListState::new(),
            search_query: String::new(),
            searching: false,
            filtered_files: Vec::new(),
            coupled_files: Vec::new(),
            coupling_list_state: ListState::new(),
            top_pairs: Vec::new(),
            functions: Vec::new(),
            function_list_state: ListState::new(),
            func_couplings: Vec::new(),
            call_edges: Vec::new(),
            active_panel: Panel::FileList,
            coupling_view: CouplingView::Combined,
            detail_level: DetailLevel::File,
            status_message: "Press q to quit, j/k to navigate, Enter to select, Tab to switch panels".to_string(),
            quit: false,
        }
    }

    /// Load initial data from the database.
    pub fn load_data(&mut self) -> Result<()> {
        self.files = self.database.list_files()
            .context("Failed to load file list")?;

        // Initialize filtered list
        self.filtered_files = (0..self.files.len()).collect();

        if !self.filtered_files.is_empty() {
            self.file_list_state.select(Some(0));
        }

        // Load top pairs
        self.top_pairs = self.database.query_top_coupled_pairs(20, 0.1, 3)
            .unwrap_or_default();

        Ok(())
    }

    /// Load coupling data for a specific file.
    pub fn load_file_coupling(&mut self, file_path: &str) {
        self.coupled_files = self.database.query_temporal_coupling(file_path, 0.1, 20)
            .unwrap_or_default();

        if !self.coupled_files.is_empty() {
            self.coupling_list_state.select(Some(0));
        } else {
            self.coupling_list_state.select(None);
        }

        // Load function data for this file
        let repo = git_engine::open_repo(&self.repo_path).ok();
        if let Some(ref repo) = repo {
            if let Ok(source) = git_engine::read_file_from_head(repo, file_path) {
                if analyzer::detect_language(file_path).is_some() {
                    self.functions = analyzer::extract_functions(file_path, &source)
                        .unwrap_or_default();

                    if !self.functions.is_empty() {
                        self.function_list_state.select(Some(0));
                    }

                    // Try to load function-level coupling
                    let first_func_name = self.functions.first().map(|f| f.name.clone());
                    if let Some(func_name) = first_func_name {
                        self.func_couplings = self.database.query_function_temporal_coupling(
                            file_path, &func_name, 0.1, 15
                        ).unwrap_or_default();
                    }

                    // Build call graph for this file
                    let mut file_sources = std::collections::HashMap::new();
                    file_sources.insert(file_path.to_string(), source);
                    // Collect coupled file paths first to avoid borrow issues
                    let coupled_paths: Vec<String> = self.coupled_files.iter()
                        .map(|c| c.file_path.clone())
                        .collect();
                    for coupled_path in coupled_paths {
                        if let Ok(src) = git_engine::read_file_from_head(repo, &coupled_path) {
                            file_sources.insert(coupled_path, src);
                        }
                    }
                    self.call_edges = analyzer::build_call_graph(&file_sources)
                        .unwrap_or_default();
                }
            }
        }

        self.status_message = format!("{} | {} coupled files | {} functions | {} call edges",
            file_path,
            self.coupled_files.len(),
            self.functions.len(),
            self.call_edges.len()
        );
    }

    // ─── Navigation ──────────────────────────────────────────────

    pub fn next_item(&mut self) {
        match self.active_panel {
            Panel::FileList => {
                let len = self.filtered_files.len();
                if len > 0 {
                    let i = match self.file_list_state.selected() {
                        Some(i) => {
                            if i >= len - 1 { 0 } else { i + 1 }
                        }
                        None => 0,
                    };
                    self.file_list_state.select(Some(i));
                    self.on_file_selected();
                }
            }
            Panel::CouplingMap => {
                let len = match self.detail_level {
                    DetailLevel::File => self.coupled_files.len(),
                    DetailLevel::Function => self.functions.len(),
                };
                if len > 0 {
                    let i = match self.coupling_list_state.selected() {
                        Some(i) => {
                            if i >= len - 1 { 0 } else { i + 1 }
                        }
                        None => 0,
                    };
                    self.coupling_list_state.select(Some(i));
                }
            }
            Panel::Details => {}
        }
    }

    pub fn prev_item(&mut self) {
        match self.active_panel {
            Panel::FileList => {
                let len = self.filtered_files.len();
                if len > 0 {
                    let i = match self.file_list_state.selected() {
                        Some(i) => {
                            if i == 0 { len - 1 } else { i - 1 }
                        }
                        None => 0,
                    };
                    self.file_list_state.select(Some(i));
                    self.on_file_selected();
                }
            }
            Panel::CouplingMap => {
                let len = match self.detail_level {
                    DetailLevel::File => self.coupled_files.len(),
                    DetailLevel::Function => self.functions.len(),
                };
                if len > 0 {
                    let i = match self.coupling_list_state.selected() {
                        Some(i) => {
                            if i == 0 { len - 1 } else { i - 1 }
                        }
                        None => 0,
                    };
                    self.coupling_list_state.select(Some(i));
                }
            }
            Panel::Details => {}
        }
    }

    pub fn select_item(&mut self) {
        match self.active_panel {
            Panel::FileList => {
                self.on_file_selected();
                self.active_panel = Panel::CouplingMap;
            }
            Panel::CouplingMap => {
                self.active_panel = Panel::Details;
            }
            Panel::Details => {
                self.active_panel = Panel::CouplingMap;
            }
        }
    }

    pub fn go_back(&mut self) {
        match self.active_panel {
            Panel::FileList => {}
            Panel::CouplingMap => {
                self.active_panel = Panel::FileList;
            }
            Panel::Details => {
                self.active_panel = Panel::CouplingMap;
            }
        }
    }

    pub fn next_panel(&mut self) {
        self.active_panel = match self.active_panel {
            Panel::FileList => Panel::CouplingMap,
            Panel::CouplingMap => Panel::Details,
            Panel::Details => Panel::FileList,
        };
    }

    pub fn prev_panel(&mut self) {
        self.active_panel = match self.active_panel {
            Panel::FileList => Panel::Details,
            Panel::CouplingMap => Panel::FileList,
            Panel::Details => Panel::CouplingMap,
        };
    }

    pub fn toggle_coupling_view(&mut self) {
        self.coupling_view = match self.coupling_view {
            CouplingView::Temporal => CouplingView::Static,
            CouplingView::Static => CouplingView::Combined,
            CouplingView::Combined => CouplingView::Temporal,
        };
        self.status_message = format!("Coupling view: {:?}", self.coupling_view);
    }

    pub fn toggle_function_view(&mut self) {
        self.detail_level = match self.detail_level {
            DetailLevel::File => DetailLevel::Function,
            DetailLevel::Function => DetailLevel::File,
        };
        self.status_message = format!("Detail level: {:?}", self.detail_level);
    }

    // ─── Search ─────────────────────────────────────────────────

    pub fn start_search(&mut self) {
        self.searching = true;
        self.search_query.clear();
        self.status_message = "Search: (type to filter, Enter to confirm)".to_string();
    }

    pub fn finish_search(&mut self) {
        self.searching = false;
        self.apply_search();
    }

    pub fn append_search(&mut self, c: char) {
        self.search_query.push(c);
        self.apply_search();
    }

    pub fn backspace_search(&mut self) {
        self.search_query.pop();
        self.apply_search();
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    fn apply_search(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_files = (0..self.files.len()).collect();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_files = self.files.iter().enumerate()
                .filter(|(_, f)| f.file_path.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }

        if !self.filtered_files.is_empty() {
            self.file_list_state.select(Some(0));
            self.on_file_selected();
        } else {
            self.file_list_state.select(None);
        }
    }

    // ─── Helpers ─────────────────────────────────────────────────

    fn on_file_selected(&mut self) {
        let file_path = self.file_list_state.selected()
            .and_then(|idx| self.filtered_files.get(idx))
            .and_then(|&file_idx| self.files.get(file_idx))
            .map(|f| f.file_path.clone());

        if let Some(path) = file_path {
            self.load_file_coupling(&path);
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Get the currently selected file path
    pub fn get_selected_file(&self) -> Option<String> {
        self.file_list_state.selected()
            .and_then(|idx| self.filtered_files.get(idx))
            .and_then(|&file_idx| self.files.get(file_idx))
            .map(|f| f.file_path.clone())
    }

    /// Get the currently selected coupled file
    pub fn get_selected_coupled_file(&self) -> Option<&db::CoupledFile> {
        self.coupling_list_state.selected()
            .and_then(|idx| self.coupled_files.get(idx))
    }

    /// Get risk level for a coupled file based on confidence and static analysis
    pub fn get_risk_for_coupled(&self, coupled: &db::CoupledFile) -> analyzer::RiskLevel {
        let has_static = self.call_edges.iter().any(|e| {
            let selected_file = self.get_selected_file().unwrap_or_default();
            (e.caller_file == selected_file && e.callee_file.as_deref() == Some(&coupled.file_path))
            || (e.callee_name != "(unknown)" && e.caller_file == coupled.file_path)
        });
        analyzer::classify_risk(has_static, coupled.confidence)
    }
}
