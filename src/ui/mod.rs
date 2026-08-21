pub mod app;
pub mod markdown;
pub mod textinput;

pub use app::{Action, App, Focus};
use textinput::InputView;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

pub use app::{ConfigView, LinkPrompt, PromptAdminView, PromptPickerView, RenamePrompt};

/// Render the full three-pane layout plus a footer.
pub fn render(f: &mut Frame, app: &mut App) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());

    let [left, center, right] = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(45),
        Constraint::Percentage(25),
    ])
    .areas(body);

    // The interface accent flips to a distinct colour while searching, as a
    // modal cue that keystrokes are going into the search box.
    let accent = if app.searching {
        Color::Yellow
    } else {
        Color::Cyan
    };

    // The center column stacks the preview over the scratchpad.
    let [preview_area, scratch_area] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(center);

    render_notes_list(f, app, left, accent);
    render_preview(f, app, preview_area, accent);
    render_scratchpad(f, app, scratch_area, accent);
    render_related(f, app, right, accent);
    render_footer(f, app, footer, accent);

    // Modals overlay everything while active.
    if let Some(prompt) = app.linking_prompt() {
        render_link_modal(f, &prompt);
    }
    if let Some(view) = app.prompt_picker_view() {
        render_prompt_modal(f, &view);
    }
    if let Some(prompt) = app.rename_prompt() {
        render_rename_modal(f, &prompt);
    }
    if let Some(view) = app.config_view() {
        render_config_modal(f, &view);
    }
    if let Some(view) = app.prompt_admin_view() {
        render_prompt_admin(f, &view);
    }
}

fn render_prompt_admin(f: &mut Frame, view: &PromptAdminView) {
    let area = centered_rect(70, 18, f.area());
    f.render_widget(Clear, area);

    let (lines, hint): (Vec<Line>, &str) = match view {
        PromptAdminView::List { prompts, selected } => {
            let mut lines: Vec<Line> = Vec::new();
            if prompts.is_empty() {
                lines.push(Line::from("(no prompts — press 'a' to add one)".dim()));
            }
            for (i, label) in prompts.iter().enumerate() {
                let marker = if i == *selected { "› " } else { "  " };
                let style = if i == *selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
            }
            (
                lines,
                "↑/↓ move · a add · e edit · d delete · J/K reorder · Esc close",
            )
        }
        PromptAdminView::Edit {
            adding,
            label,
            text,
            editing_label,
        } => {
            let label_line = if *editing_label {
                input_spans(vec![Span::from("Label  ").fg(Color::Cyan)], label)
            } else {
                Line::from(vec![
                    Span::from("Label  ").dim(),
                    Span::from(label.text.clone()),
                ])
            };
            let text_line = if *editing_label {
                Line::from(vec![
                    Span::from("Prompt ").dim(),
                    Span::from(text.text.clone()),
                ])
            } else {
                input_spans(vec![Span::from("Prompt ").fg(Color::Cyan)], text)
            };
            let heading = if *adding { "Add prompt" } else { "Edit prompt" };
            (
                vec![
                    Line::from(heading.bold()),
                    Line::from(""),
                    label_line,
                    Line::from(""),
                    text_line,
                ],
                "Tab switch field · Enter save · Esc cancel",
            )
        }
    };

    let mut all = lines;
    all.push(Line::from(""));
    all.push(Line::from(hint.dim()));

    let modal = Paragraph::new(all)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Prompt library ")
                .border_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(modal, area);
}

fn render_config_modal(f: &mut Frame, view: &ConfigView) {
    let area = centered_rect(60, 11, f.area());
    f.render_widget(Clear, area);

    // One line per row; the selected row is marked. The model row shows a live
    // cursor (via `input_spans`) while it is being edited.
    let rows = [
        format!("Provider   {}", view.provider_label),
        format!("Model      {}", view.model),
        match view.effort {
            Some(label) => format!("Effort     {label}"),
            None => "Effort     — (not supported)".to_string(),
        },
        format!("Prompts    {} preset(s)", view.prompt_count),
    ];
    const MODEL_ROW: usize = 1;
    let mut lines: Vec<Line> = Vec::new();
    for (i, text) in rows.iter().enumerate() {
        let marker = if i == view.selected { "› " } else { "  " };
        let style = if i == view.selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        if i == MODEL_ROW {
            if let Some(input) = &view.model_input {
                lines.push(input_spans(
                    vec![Span::styled(format!("{marker}Model      "), style)],
                    input,
                ));
                continue;
            }
        }
        lines.push(Line::from(Span::styled(format!("{marker}{text}"), style)));
    }
    lines.push(Line::from(""));
    let hint = if view.editing_model {
        "Type a model name · Enter save · Esc cancel"
    } else {
        "↑/↓ move · ←/→ change provider/effort · Enter edit · Esc close"
    };
    lines.push(Line::from(hint.dim()));

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" LLM configuration ")
                .border_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(modal, area);
}

fn render_rename_modal(f: &mut Frame, prompt: &RenamePrompt) {
    let area = centered_rect(60, 7, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from("Rename note (Enter to confirm · Esc to cancel):".dim()),
        Line::from("The timestamp is kept; only the name after it changes.".dim()),
        Line::from(""),
        input_spans(
            vec![Span::from(format!("{}-", prompt.prefix)).fg(Color::DarkGray)],
            &prompt.slug,
        ),
    ];
    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Rename note ")
                .border_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(modal, area);
}

fn render_prompt_modal(f: &mut Frame, view: &PromptPickerView) {
    match view {
        PromptPickerView::List { labels, selected } => {
            let area = centered_rect(56, labels.len() as u16 + 4, f.area());
            f.render_widget(Clear, area);
            let items: Vec<ListItem> = labels
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let key = if i < 9 {
                        format!("[{}] ", i + 1)
                    } else {
                        "    ".to_string()
                    };
                    ListItem::new(format!("{key}{label}"))
                })
                .collect();
            let mut state = ratatui::widgets::ListState::default();
            state.select(Some(*selected));
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Send scratchpad to LLM ")
                        .border_style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                )
                .highlight_style(selection_highlight())
                .highlight_symbol("› ");
            f.render_stateful_widget(list, area, &mut state);
        }
        PromptPickerView::Custom { input } => {
            let area = centered_rect(60, 6, f.area());
            f.render_widget(Clear, area);
            let lines = vec![
                Line::from("Custom prompt (Enter to send · Esc to cancel):".dim()),
                Line::from(""),
                input_spans(vec![], input),
            ];
            let modal = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Send scratchpad to LLM ")
                        .border_style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(modal, area);
        }
    }
}

/// Build the spans for an editable field: an optional prefix, the text, and a
/// block cursor drawn (reverse-video) at the input's cursor position.
fn input_spans(prefix: Vec<Span<'static>>, input: &InputView) -> Line<'static> {
    let chars: Vec<char> = input.text.chars().collect();
    let cursor = input.cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let (at, after): (String, String) = if cursor < chars.len() {
        (
            chars[cursor].to_string(),
            chars[cursor + 1..].iter().collect(),
        )
    } else {
        // Cursor past the end: draw it on a trailing space.
        (" ".to_string(), String::new())
    };
    let mut spans = prefix;
    spans.push(Span::from(before));
    spans.push(Span::styled(
        at,
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::from(after));
    Line::from(spans)
}

/// A `Rect` of the given size, centered within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn render_link_modal(f: &mut Frame, prompt: &LinkPrompt) {
    let area = centered_rect(62, 11, f.area());
    f.render_widget(Clear, area);

    let lines = vec![
        Line::from(format!(
            "Pair {}/{} — link these notes?",
            prompt.index, prompt.total
        ))
        .bold(),
        Line::from(""),
        Line::from(vec![
            Span::from("A: ").dim(),
            Span::raw(prompt.a_name.clone()),
        ]),
        Line::from(vec![
            Span::from("B: ").dim(),
            Span::raw(prompt.b_name.clone()),
        ]),
        Line::from(""),
        Line::from("[1] A → B      [2] B → A      [3] both"),
        Line::from("[4] skip       [Esc] cancel"),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Link notes ")
                .border_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(modal, area);
}

fn selection_highlight() -> Style {
    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// A bordered block whose border is highlighted, in `accent`, when its pane has
/// focus.
fn pane_block(title: String, focused: bool, accent: Color) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).title(title);
    if focused {
        block.border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
    } else {
        block
    }
}

/// The left column (pane 1): the notes list, by filename.
fn render_notes_list(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    let focused = app.focus == Focus::Notes;
    let files = app.visible_files();
    let title = if app.searching || !app.query_is_empty() {
        // The block title can't host styled spans, so render the cursor as a
        // caret inserted at its character offset.
        let view = app.query_view();
        let chars: Vec<char> = view.text.chars().collect();
        let cursor = view.cursor.min(chars.len());
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        format!(" 1 Search: {before}▏{after} ")
    } else {
        format!(" 1 Files ({}) ", files.len())
    };

    let items: Vec<ListItem> = files.into_iter().map(ListItem::new).collect();
    let list = List::new(items)
        .block(pane_block(title, focused, accent))
        .highlight_style(selection_highlight())
        .highlight_symbol("› ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_preview(f: &mut Frame, app: &App, area: Rect, accent: Color) {
    let focused = app.focus == Focus::Preview;
    let title = match app.active_note() {
        Some(note) => format!(" 2 {} ", note.title),
        None => " 2 Preview ".to_string(),
    };

    let mut block = pane_block(title, focused, accent);
    if let Some(rel) = app.current_relative_path() {
        block = block.title_bottom(Line::from(format!(" {rel} ").dim()).right_aligned());
    }

    let paragraph = Paragraph::new(app.preview().clone())
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));

    f.render_widget(paragraph, area);
}

fn render_related(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    let focused = app.focus == Focus::Related;
    let related = app.related();
    let title = format!(" 3 Related ({}) ", related.len());

    if related.is_empty() {
        let empty = Paragraph::new("No related notes.".dim())
            .block(pane_block(title, focused, accent))
            .wrap(Wrap { trim: true });
        f.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = related
        .iter()
        .map(|r| {
            ListItem::new(vec![
                Line::from(format!("• {}", r.title)),
                Line::from(format!("  {}", r.reason).dim()),
            ])
        })
        .collect();

    let list = List::new(items)
        .block(pane_block(title, focused, accent))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
        .highlight_symbol("› ");

    f.render_stateful_widget(list, area, &mut app.related_state);
}

fn render_scratchpad(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    // Two columns: the file list on the left, the command menu on the right.
    let [files_area, commands_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);

    let focused = app.focus == Focus::Scratchpad;
    let files = app.scratchpad_files();
    let title = format!(" 4 Scratchpad ({}) ", files.len());

    if files.is_empty() {
        let empty = Paragraph::new("Empty — press a to add.".dim())
            .block(pane_block(title, focused, accent))
            .wrap(Wrap { trim: true });
        f.render_widget(empty, files_area);
    } else {
        let items: Vec<ListItem> = files.into_iter().map(ListItem::new).collect();
        let list = List::new(items)
            .block(pane_block(title, focused, accent))
            .highlight_style(selection_highlight())
            .highlight_symbol("› ");
        f.render_stateful_widget(list, files_area, &mut app.scratchpad_state);
    }

    render_scratchpad_commands(f, commands_area, accent);
}

fn render_scratchpad_commands(f: &mut Frame, area: Rect, accent: Color) {
    let key = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(vec![Span::styled("(l)", key), Span::raw(" Link notes")]),
        Line::from(vec![Span::styled("(s)", key), Span::raw(" Send to LLM")]),
    ];
    let commands =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Commands "));
    f.render_widget(commands, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect, accent: Color) {
    // A transient status message takes precedence over the key hints.
    if let Some(status) = &app.status {
        f.render_widget(Paragraph::new(Line::from(status.clone().bold())), area);
        return;
    }

    // While searching, the footer stands out in the accent colour as another
    // modal cue.
    if app.searching {
        let text = " SEARCH   type to filter   Enter/Esc: apply   Esc again: clear";
        let style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
        f.render_widget(Paragraph::new(Line::styled(text, style)), area);
        return;
    }

    let move_hint = match app.focus {
        Focus::Notes => "j/k: select",
        Focus::Preview => "j/k: scroll",
        Focus::Related => "j/k: select   Enter: jump",
        Focus::Scratchpad => "j/k: select   x: remove",
    };
    let text = if !app.query_is_empty() {
        format!(
            "filtered: \"{}\"   Esc: clear   {move_hint}   /: search   q: quit",
            app.query_text()
        )
    } else {
        format!("1-4/Tab: panes   {move_hint}   a: +scratchpad   e: edit   r: rename   c: config   y: copy   /: search   q: quit")
    };
    f.render_widget(Paragraph::new(Line::from(text).dim()), area);
}
