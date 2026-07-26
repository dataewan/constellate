pub mod app;
pub mod markdown;

pub use app::{Action, App, Focus};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

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

    render_notes_list(f, app, left, accent);
    render_preview(f, app, center, accent);
    render_related(f, app, right, accent);
    render_footer(f, app, footer, accent);
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
    let title = if app.searching || !app.query.is_empty() {
        format!(" 1 Search: {}▏ ", app.query)
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
    };
    let text = if !app.query.is_empty() {
        format!("filtered: \"{}\"   Esc: clear   {move_hint}   /: search   q: quit", app.query)
    } else {
        format!("1/2/3·Tab: panes   {move_hint}   e: edit   y: copy   /: search   q: quit")
    };
    f.render_widget(Paragraph::new(Line::from(text).dim()), area);
}
