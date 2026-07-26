pub mod app;

pub use app::{Action, App};

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
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

    render_list(f, app, left);
    render_preview(f, app, center);
    render_related(f, right);
    render_footer(f, app, footer);
}

fn render_list(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let title = if app.searching || !app.query.is_empty() {
        format!(" Search: {}▏ ", app.query)
    } else {
        format!(" Notes ({}) ", app.visible_titles().len())
    };

    let items: Vec<ListItem> = app
        .visible_titles()
        .into_iter()
        .map(|t| ListItem::new(t.to_string()))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("› ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_preview(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default().borders(Borders::ALL);
    let block = match app.current_note() {
        Some(note) => {
            let mut b = block.title(format!(" {} ", note.title));
            if let Some(rel) = app.current_relative_path() {
                b = b.title_bottom(Line::from(format!(" {rel} ").dim()).right_aligned());
            }
            b
        }
        None => block.title(" Preview "),
    };

    let content = app
        .current_note()
        .map(|n| n.content.clone())
        .unwrap_or_else(|| "No note selected.".to_string());

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn render_related(f: &mut Frame, area: ratatui::layout::Rect) {
    let placeholder = Paragraph::new(vec![
        Line::from("Related notes".bold()),
        Line::from(""),
        Line::from(Span::from("Arriving in Phase 2:").dim()),
        Line::from(Span::from("• shared [[wikilinks]]").dim()),
        Line::from(Span::from("• shared tags").dim()),
        Line::from(Span::from("• keyword overlap").dim()),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Context "))
    .wrap(Wrap { trim: true });

    f.render_widget(placeholder, area);
}

fn render_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if app.searching {
        "type to filter   Enter: apply   Esc: clear"
    } else {
        "j/k: move   /: search   e/Enter: edit   q: quit"
    };
    let footer = Paragraph::new(Line::from(text).dim());
    f.render_widget(footer, area);
}
