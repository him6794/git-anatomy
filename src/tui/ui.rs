//! TUI Rendering: Draw the interactive terminal interface
//!
//! Layout:
//! ┌──────────────────────────────────────────────────────────────────┐
//! │  git-anatomy ◈ Blast Radius Explorer              [q]uit [?]help│
//! ├──────────────────────┬───────────────────────────────────────────┤
//! │  📁 FILES            │  🔗 COUPLING MAP                          │
//! │  ...file list...     │  ...coupled files with risk levels...    │
//! │                      │                                           │
//! ├──────────────────────┴───────────────────────────────────────────┤
//! │  📋 DETAILS: risk assessment, commit history, call chains       │
//! └──────────────────────────────────────────────────────────────────┘

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use super::app::{App, CouplingView, DetailLevel, Panel};
use crate::analyzer;

// ─── Color Palette ───────────────────────────────────────────────────────────

const COLOR_BG: Color = Color::Rgb(30, 30, 46);       // Catppuccin Mocha base
const COLOR_SURFACE: Color = Color::Rgb(49, 50, 68);   // Catppuccin Mocha surface0
const COLOR_OVERLAY: Color = Color::Rgb(69, 71, 90);   // Catppuccin Mocha surface1
const COLOR_TEXT: Color = Color::Rgb(205, 214, 244);    // Catppuccin Mocha text
const COLOR_SUBTEXT: Color = Color::Rgb(166, 173, 200); // Catppuccin Mocha subtext0
const COLOR_RED: Color = Color::Rgb(243, 139, 168);     // Catppuccin Mocha red
const COLOR_PEACH: Color = Color::Rgb(250, 179, 135);   // Catppuccin Mocha peach
const COLOR_YELLOW: Color = Color::Rgb(249, 226, 175);  // Catppuccin Mocha yellow
const COLOR_GREEN: Color = Color::Rgb(166, 227, 161);   // Catppuccin Mocha green
const COLOR_BLUE: Color = Color::Rgb(137, 180, 250);    // Catppuccin Mocha blue
const COLOR_MAUVE: Color = Color::Rgb(203, 166, 247);   // Catppuccin Mocha mauve
const COLOR_TEAL: Color = Color::Rgb(148, 226, 213);    // Catppuccin Mocha teal
const COLOR_LAVENDER: Color = Color::Rgb(180, 190, 254);// Catppuccin Mocha lavender

// ─── Main Draw Function ─────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();

    // Background
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_BG)),
        size,
    );

    // Main layout: top bar + content + status bar
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),   // Title bar
            Constraint::Min(5),      // Main content
            Constraint::Length(1),   // Status bar
        ])
        .split(size);

    draw_title_bar(f, app, main_layout[0]);
    draw_main_content(f, app, main_layout[1]);
    draw_status_bar(f, app, main_layout[2]);
}

// ─── Title Bar ───────────────────────────────────────────────────────────────

fn draw_title_bar(f: &mut Frame, _app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(" ◈ ", Style::default().fg(COLOR_MAUVE).add_modifier(Modifier::BOLD)),
        Span::styled("git-anatomy", Style::default().fg(COLOR_LAVENDER).add_modifier(Modifier::BOLD)),
        Span::styled(" — Blast Radius Explorer", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("  │  ", Style::default().fg(COLOR_OVERLAY)),
        Span::styled("[q]uit ", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("[j/k]nav ", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("[Enter]select ", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("[Tab]panel ", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("[/]search ", Style::default().fg(COLOR_SUBTEXT)),
        Span::styled("[c]oupling [f]unction", Style::default().fg(COLOR_SUBTEXT)),
    ]);
    f.render_widget(
        Paragraph::new(title).style(Style::default().bg(COLOR_SURFACE)),
        area,
    );
}

// ─── Main Content ────────────────────────────────────────────────────────────

fn draw_main_content(f: &mut Frame, app: &App, area: Rect) {
    // Split into: file list (left) | coupling map (right top) + details (right bottom)
    let content_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(40),  // File list
            Constraint::Min(40),     // Right side
        ])
        .split(area);

    draw_file_list(f, app, content_layout[0]);

    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(55), // Coupling map
            Constraint::Percentage(45), // Details
        ])
        .split(content_layout[1]);

    draw_coupling_map(f, app, right_layout[0]);
    draw_details(f, app, right_layout[1]);
}

// ─── File List Panel ─────────────────────────────────────────────────────────

fn draw_file_list(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.active_panel == Panel::FileList;
    let border_color = if is_focused { COLOR_MAUVE } else { COLOR_OVERLAY };
    let title_style = if is_focused {
        Style::default().fg(COLOR_MAUVE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_SUBTEXT)
    };

    let items: Vec<ListItem> = app.filtered_files.iter()
        .enumerate()
        .map(|(idx, &file_idx)| {
            let file = &app.files[file_idx];
            let is_selected = app.file_list_state.selected() == Some(idx);

            let icon = if analyzer::detect_language(&file.file_path).is_some() {
                "ƒ"
            } else {
                " "
            };

            let style = if is_selected {
                Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_SUBTEXT)
            };

            let line = Line::from(vec![
                Span::styled(format!(" {} ", icon), Style::default().fg(COLOR_TEAL)),
                Span::styled(
                    truncate_str(&file.file_path, area.width as usize - 8),
                    style,
                ),
                Span::styled(
                    format!(" ({})", file.commit_count),
                    Style::default().fg(COLOR_OVERLAY),
                ),
            ]);

            ListItem::new(line).style(Style::default().bg(
                if is_selected { COLOR_SURFACE } else { COLOR_BG }
            ))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(border_color))
                .title(Line::from(vec![
                    Span::styled(" 📁 ", Style::default().fg(COLOR_BLUE)),
                    Span::styled("FILES", title_style),
                    if !app.search_query.is_empty() {
                        Span::styled(format!(" [{}]", app.search_query), Style::default().fg(COLOR_YELLOW))
                    } else {
                        Span::raw("")
                    },
                ]))
                .style(Style::default().bg(COLOR_BG))
        );

    f.render_widget(list, area);
}

// ─── Coupling Map Panel ──────────────────────────────────────────────────────

fn draw_coupling_map(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.active_panel == Panel::CouplingMap;
    let border_color = if is_focused { COLOR_MAUVE } else { COLOR_OVERLAY };
    let title_style = if is_focused {
        Style::default().fg(COLOR_MAUVE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_SUBTEXT)
    };

    let view_label = match app.coupling_view {
        CouplingView::Temporal => "TEMPORAL",
        CouplingView::Static => "STATIC",
        CouplingView::Combined => "COMBINED",
    };

    let detail_label = match app.detail_level {
        DetailLevel::File => "FILE",
        DetailLevel::Function => "FUNC",
    };

    match app.detail_level {
        DetailLevel::File => draw_file_coupling(f, app, area, border_color, title_style, view_label, detail_label, is_focused),
        DetailLevel::Function => draw_function_coupling(f, app, area, border_color, title_style, view_label, detail_label, is_focused),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_file_coupling(
    f: &mut Frame,
    app: &App,
    area: Rect,
    border_color: Color,
    title_style: Style,
    view_label: &str,
    detail_label: &str,
    is_focused: bool,
) {
    let selected_file = app.get_selected_file().unwrap_or_default();

    // Header
    let header = Line::from(vec![
        Span::styled(" # ", Style::default().fg(COLOR_OVERLAY)),
        Span::styled(format!("{:<40}", "File Path"), Style::default().fg(COLOR_SUBTEXT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>8}", "Conf%"), Style::default().fg(COLOR_SUBTEXT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>6}", "CoChg"), Style::default().fg(COLOR_SUBTEXT).add_modifier(Modifier::BOLD)),
        Span::styled(" Risk ", Style::default().fg(COLOR_SUBTEXT).add_modifier(Modifier::BOLD)),
    ]);

    let mut lines: Vec<Line> = vec![header];

    for (idx, coupled) in app.coupled_files.iter().enumerate() {
        let is_selected = app.coupling_list_state.selected() == Some(idx) && is_focused;
        let risk = app.get_risk_for_coupled(coupled);

        let (risk_icon, risk_color, risk_label) = match risk {
            analyzer::RiskLevel::High => ("●", COLOR_RED, "HIGH"),
            analyzer::RiskLevel::Medium => ("●", COLOR_PEACH, "MED "),
            analyzer::RiskLevel::Low => ("●", COLOR_YELLOW, "LOW "),
        };

        let confidence_pct = coupled.confidence * 100.0;
        let conf_color = if confidence_pct >= 70.0 {
            COLOR_RED
        } else if confidence_pct >= 50.0 {
            COLOR_PEACH
        } else {
            COLOR_GREEN
        };

        let bg = if is_selected { COLOR_SURFACE } else { COLOR_BG };

        let line = Line::from(vec![
            Span::styled(format!(" {:>2} ", idx + 1), Style::default().fg(COLOR_OVERLAY).bg(bg)),
            Span::styled(
                format!("{:<40}", truncate_str(&coupled.file_path, 40)),
                Style::default().fg(COLOR_TEXT).bg(bg),
            ),
            Span::styled(
                format!("{:>7.1}%", confidence_pct),
                Style::default().fg(conf_color).add_modifier(Modifier::BOLD).bg(bg),
            ),
            Span::styled(
                format!("{:>5}", coupled.co_commit_count),
                Style::default().fg(COLOR_TEAL).bg(bg),
            ),
            Span::styled(format!(" {} ", risk_icon), Style::default().fg(risk_color).bg(bg)),
            Span::styled(risk_label, Style::default().fg(risk_color).bg(bg)),
        ]);

        lines.push(line);
    }

    if app.coupled_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No coupling data. Select a file from the left panel.",
            Style::default().fg(COLOR_SUBTEXT),
        )));
    }

    // Build the block with a coupling bar visualization
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(" 🔗 ", Style::default().fg(COLOR_TEAL)),
            Span::styled("COUPLING MAP", title_style),
            Span::styled(format!(" [{}|{}]", view_label, detail_label), Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(format!(" — {}", truncate_str(&selected_file, 30)), Style::default().fg(COLOR_BLUE)),
        ]))
        .style(Style::default().bg(COLOR_BG));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

#[allow(clippy::too_many_arguments)]
fn draw_function_coupling(
    f: &mut Frame,
    app: &App,
    area: Rect,
    border_color: Color,
    title_style: Style,
    view_label: &str,
    detail_label: &str,
    is_focused: bool,
) {
    let selected_file = app.get_selected_file().unwrap_or_default();

    let mut lines: Vec<Line> = Vec::new();

    // Show functions in the selected file
    for (idx, func) in app.functions.iter().enumerate() {
        let is_selected = app.function_list_state.selected() == Some(idx) && is_focused;

        let bg = if is_selected { COLOR_SURFACE } else { COLOR_BG };
        let fg = if is_selected { COLOR_TEXT } else { COLOR_SUBTEXT };

        let line = Line::from(vec![
            Span::styled(" ƒ ".to_string(), Style::default().fg(COLOR_TEAL).bg(bg)),
            Span::styled(
                format!("{:<30}", truncate_str(&func.name, 30)),
                Style::default().fg(fg).add_modifier(Modifier::BOLD).bg(bg),
            ),
            Span::styled(
                format!(" L{}-L{} ", func.start_line, func.end_line),
                Style::default().fg(COLOR_OVERLAY).bg(bg),
            ),
        ]);

        lines.push(line);
    }

    // Show function-level coupling
    if !app.func_couplings.is_empty() {
        lines.push(Line::from(Span::styled(
            "  ── Function Coupling ──",
            Style::default().fg(COLOR_OVERLAY),
        )));

        for coupled in &app.func_couplings {
            let has_static = app.call_edges.iter().any(|e| e.callee_name == coupled.function_name);
            let risk = analyzer::classify_risk(has_static, coupled.confidence);

            let (risk_icon, risk_color) = match risk {
                analyzer::RiskLevel::High => ("●", COLOR_RED),
                analyzer::RiskLevel::Medium => ("●", COLOR_PEACH),
                analyzer::RiskLevel::Low => ("●", COLOR_YELLOW),
            };

            let line = Line::from(vec![
                Span::styled("   → ", Style::default().fg(COLOR_OVERLAY)),
                Span::styled(
                    format!("{:<25}", truncate_str(&coupled.function_name, 25)),
                    Style::default().fg(COLOR_TEXT),
                ),
                Span::styled(
                    format!(" {}::{:<20}", truncate_str(&coupled.file_path, 10), truncate_str(&coupled.function_name, 20)),
                    Style::default().fg(COLOR_SUBTEXT),
                ),
                Span::styled(
                    format!(" {:.0}%", coupled.confidence * 100.0),
                    Style::default().fg(COLOR_TEAL),
                ),
                Span::styled(format!(" {}", risk_icon), Style::default().fg(risk_color)),
            ]);

            lines.push(line);
        }
    }

    if app.functions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No functions found. Select a source file.",
            Style::default().fg(COLOR_SUBTEXT),
        )));
    }

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(" ƒ ", Style::default().fg(COLOR_TEAL)),
            Span::styled("FUNCTION VIEW", title_style),
            Span::styled(format!(" [{}|{}]", view_label, detail_label), Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(format!(" — {}", truncate_str(&selected_file, 30)), Style::default().fg(COLOR_BLUE)),
        ]))
        .style(Style::default().bg(COLOR_BG));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

// ─── Details Panel ───────────────────────────────────────────────────────────

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.active_panel == Panel::Details;
    let title_style = if is_focused {
        Style::default().fg(COLOR_MAUVE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_SUBTEXT)
    };

    let mut lines: Vec<Line> = Vec::new();

    // Get selected file path upfront (owned)
    let selected_file = app.get_selected_file().unwrap_or_default();

    if let Some(coupled) = app.get_selected_coupled_file() {
        let risk = app.get_risk_for_coupled(coupled);

        let (risk_label, risk_color) = match risk {
            analyzer::RiskLevel::High => ("HIGH", COLOR_RED),
            analyzer::RiskLevel::Medium => ("MEDIUM", COLOR_PEACH),
            analyzer::RiskLevel::Low => ("LOW", COLOR_YELLOW),
        };

        // Risk badge
        lines.push(Line::from(vec![
            Span::styled(" Risk: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(format!(" {} ", risk_label), Style::default().fg(risk_color).add_modifier(Modifier::BOLD)),
            Span::styled("  │  ", Style::default().fg(COLOR_OVERLAY)),
            Span::styled("Confidence: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(format!("{:.1}%", coupled.confidence * 100.0), Style::default().fg(COLOR_TEAL).add_modifier(Modifier::BOLD)),
            Span::styled("  │  ", Style::default().fg(COLOR_OVERLAY)),
            Span::styled("Co-commits: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(format!("{}", coupled.co_commit_count), Style::default().fg(COLOR_TEXT)),
        ]));

        // File path
        let coupled_file_path = coupled.file_path.clone();
        lines.push(Line::from(vec![
            Span::styled(" File: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(coupled_file_path.clone(), Style::default().fg(COLOR_BLUE)),
        ]));

        // Confidence bar
        let bar_width = (area.width as usize).saturating_sub(20);
        let filled = ((coupled.confidence * bar_width as f64) as usize).min(bar_width);
        let bar_color = if coupled.confidence >= 0.7 {
            COLOR_RED
        } else if coupled.confidence >= 0.5 {
            COLOR_PEACH
        } else {
            COLOR_GREEN
        };

        let bar_str = format!("{}{}",
            "█".repeat(filled),
            "░".repeat(bar_width - filled),
        );
        lines.push(Line::from(vec![
            Span::styled(" Conf: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(bar_str, Style::default().fg(bar_color)),
        ]));

        // Static dependency info
        let has_static = app.call_edges.iter().any(|e| {
            e.caller_file == selected_file && e.callee_file.as_deref() == Some(&coupled_file_path)
        });

        lines.push(Line::from(vec![
            Span::styled(" Static dep: ", Style::default().fg(COLOR_SUBTEXT)),
            if has_static {
                Span::styled("YES ✓", Style::default().fg(COLOR_RED).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("no ✗", Style::default().fg(COLOR_OVERLAY))
            },
            Span::styled("  │  ", Style::default().fg(COLOR_OVERLAY)),
            Span::styled("Temporal: ", Style::default().fg(COLOR_SUBTEXT)),
            if coupled.confidence >= 0.7 {
                Span::styled("STRONG", Style::default().fg(COLOR_RED).add_modifier(Modifier::BOLD))
            } else if coupled.confidence >= 0.5 {
                Span::styled("MODERATE", Style::default().fg(COLOR_PEACH))
            } else {
                Span::styled("WEAK", Style::default().fg(COLOR_GREEN))
            },
        ]));

        // Call chain info - clone the edge data to avoid lifetime issues
        let related_edges: Vec<_> = app.call_edges.iter()
            .filter(|e| {
                (e.caller_file == selected_file && e.callee_file.as_deref() == Some(&coupled_file_path))
                || (e.caller_file == coupled_file_path && e.callee_file.as_deref() == Some(&selected_file))
            })
            .map(|e| (e.caller_name.clone(), e.callee_name.clone()))
            .collect();

        if !related_edges.is_empty() {
            lines.push(Line::from(Span::styled(
                " Call chain:",
                Style::default().fg(COLOR_SUBTEXT),
            )));
            for (caller, callee) in related_edges.into_iter().take(5) {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(caller, Style::default().fg(COLOR_BLUE)),
                    Span::styled(" → ", Style::default().fg(COLOR_OVERLAY)),
                    Span::styled(callee, Style::default().fg(COLOR_TEAL)),
                ]));
            }
        }

        // Risk explanation
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" Interpretation: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(match risk {
                analyzer::RiskLevel::High => "Both statically dependent AND frequently co-modified. Changes here are very likely to break this file.",
                analyzer::RiskLevel::Medium => "No direct code dependency, but Git history shows these files change together. Likely hidden business coupling.",
                analyzer::RiskLevel::Low => "Code dependency exists but files are rarely modified together. Lower risk but still worth verifying.",
            }, Style::default().fg(COLOR_TEXT)),
        ]));

    } else if !selected_file.is_empty() {
        // Show file summary when no coupled file is selected
        lines.push(Line::from(vec![
            Span::styled(" Selected: ", Style::default().fg(COLOR_SUBTEXT)),
            Span::styled(selected_file.clone(), Style::default().fg(COLOR_BLUE)),
        ]));

        if !app.functions.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" Functions: {}", app.functions.len()),
                Style::default().fg(COLOR_SUBTEXT),
            )));
        }

        if !app.call_edges.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" Call edges: {}", app.call_edges.len()),
                Style::default().fg(COLOR_SUBTEXT),
            )));
        }

        // Show top coupled pairs as summary
        if !app.top_pairs.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Top Repository Couplings:",
                Style::default().fg(COLOR_SUBTEXT),
            )));
            for pair in app.top_pairs.iter().take(5) {
                let conf_color = if pair.confidence >= 0.7 { COLOR_RED } else if pair.confidence >= 0.5 { COLOR_PEACH } else { COLOR_GREEN };
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(truncate_str(&pair.file_a, 25), Style::default().fg(COLOR_TEXT)),
                    Span::styled(" ⟷ ", Style::default().fg(COLOR_OVERLAY)),
                    Span::styled(truncate_str(&pair.file_b, 25), Style::default().fg(COLOR_TEXT)),
                    Span::styled(format!(" {:.0}%", pair.confidence * 100.0), Style::default().fg(conf_color)),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  Select a file from the left panel to begin analysis.",
            Style::default().fg(COLOR_SUBTEXT),
        )));
    }

    let block = Block::default()
        .borders(Borders::NONE)
        .title(Line::from(vec![
            Span::styled(" 📋 ", Style::default().fg(COLOR_PEACH)),
            Span::styled("DETAILS", title_style),
        ]))
        .style(Style::default().bg(COLOR_BG));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

// ─── Status Bar ──────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let search_indicator = if app.searching {
        format!(" SEARCH: {}_", app.search_query)
    } else {
        String::new()
    };

    let panel_indicator = match app.active_panel {
        Panel::FileList => "FILES",
        Panel::CouplingMap => "COUPLING",
        Panel::Details => "DETAILS",
    };

    let line = Line::from(vec![
        Span::styled(format!(" {} ", panel_indicator), Style::default().fg(COLOR_MAUVE).add_modifier(Modifier::BOLD)),
        Span::styled(" │ ", Style::default().fg(COLOR_OVERLAY)),
        Span::styled(&app.status_message, Style::default().fg(COLOR_SUBTEXT)),
        if !search_indicator.is_empty() {
            Span::styled(search_indicator, Style::default().fg(COLOR_YELLOW).add_modifier(Modifier::BOLD))
        } else {
            Span::raw("")
        },
    ]);

    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(COLOR_SURFACE)),
        area,
    );
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("...{}", &s[s.len() - max_len + 3..])
    } else {
        s.chars().take(max_len).collect()
    }
}
