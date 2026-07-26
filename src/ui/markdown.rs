//! Render Markdown to styled ratatui `Text` for the preview pane.
//!
//! This is structural syntax highlighting via `pulldown-cmark` (headings,
//! emphasis, inline code, fenced code blocks, lists, quotes, links) — no
//! per-language token highlighting.

use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Convert Markdown source into styled, owned `Text`.
pub fn render(markdown: &str) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let mut renderer = Renderer::default();
    for event in Parser::new_ext(markdown, options) {
        renderer.handle(event);
    }
    renderer.finish()
}

#[derive(Default)]
struct Renderer {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    bold: bool,
    italic: bool,
    strike: bool,
    heading: Option<HeadingLevel>,
    link: Option<String>,
    in_code_block: bool,
    code_buf: String,
    code_lang: String,
    /// One entry per open list; `Some(n)` is the next ordered index.
    list_stack: Vec<Option<u64>>,
    quote_depth: usize,
}

impl Renderer {
    fn handle(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.in_code_block {
                    self.code_buf.push_str(&text);
                } else {
                    self.push(text.into_string(), self.inline_style());
                }
            }
            Event::Code(text) => {
                self.push(text.into_string(), Style::default().fg(Color::LightRed));
            }
            // Treat soft and hard breaks as line breaks so the author's layout
            // is preserved and scrolling stays line-granular.
            Event::SoftBreak | Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(48),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if self.quote_depth > 0 {
                    self.push_quote_marker();
                }
            }
            Tag::Heading { level, .. } => self.heading = Some(level),
            Tag::Strong => self.bold = true,
            Tag::Emphasis => self.italic = true,
            Tag::Strikethrough => self.strike = true,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.into_string()),
            Tag::CodeBlock(kind) => {
                self.flush_line();
                self.in_code_block = true;
                self.code_buf.clear();
                self.code_lang.clear();
                if let CodeBlockKind::Fenced(lang) = kind {
                    if !lang.is_empty() {
                        self.code_lang = lang.to_string();
                        self.lines.push(Line::from(Span::styled(
                            format!("❯ {lang}"),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
            Tag::List(first) => self.list_stack.push(first),
            Tag::Item => self.start_item(),
            Tag::BlockQuote(..) => self.quote_depth += 1,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
                self.push_blank();
            }
            TagEnd::Heading(_) => {
                self.flush_line();
                self.push_blank();
                self.heading = None;
            }
            TagEnd::Strong => self.bold = false,
            TagEnd::Emphasis => self.italic = false,
            TagEnd::Strikethrough => self.strike = false,
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    // Show the destination for web links; intra-vault targets
                    // are usually obvious from the link text.
                    if url.starts_with("http") {
                        self.spans.push(Span::styled(
                            format!(" ({url})"),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
            }
            TagEnd::CodeBlock => {
                let code = std::mem::take(&mut self.code_buf);
                let lang = std::mem::take(&mut self.code_lang);
                self.lines.extend(highlight_code(&lang, &code));
                self.in_code_block = false;
                self.push_blank();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.push_blank();
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::BlockQuote(..) => self.quote_depth = self.quote_depth.saturating_sub(1),
            _ => {}
        }
    }

    fn start_item(&mut self) {
        self.flush_line();
        let depth = self.list_stack.len();
        let indent = "  ".repeat(depth.saturating_sub(1));
        let marker = match self.list_stack.last_mut() {
            Some(Some(n)) => {
                let m = format!("{indent}{n}. ");
                *n += 1;
                m
            }
            _ => format!("{indent}• "),
        };
        self.spans
            .push(Span::styled(marker, Style::default().fg(Color::DarkGray)));
    }

    fn push_quote_marker(&mut self) {
        self.spans.push(Span::styled(
            "│ ".repeat(self.quote_depth),
            Style::default().fg(Color::DarkGray),
        ));
    }

    fn inline_style(&self) -> Style {
        if let Some(level) = self.heading {
            return Style::default()
                .fg(heading_color(level))
                .add_modifier(Modifier::BOLD);
        }
        let mut style = Style::default();
        if self.link.is_some() {
            style = style
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED);
        }
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.quote_depth > 0 {
            style = style.add_modifier(Modifier::DIM);
        }
        style
    }

    fn push(&mut self, text: String, style: Style) {
        if !text.is_empty() {
            self.spans.push(Span::styled(text, style));
        }
    }

    fn flush_line(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        self.lines.push(Line::from(spans));
    }

    /// Append a blank separator line unless the last line is already blank.
    fn push_blank(&mut self) {
        let last_blank = self
            .lines
            .last()
            .map(|l| l.spans.is_empty())
            .unwrap_or(true);
        if !last_blank {
            self.lines.push(Line::default());
        }
    }

    fn finish(mut self) -> Text<'static> {
        if !self.spans.is_empty() {
            self.flush_line();
        }
        Text::from(self.lines)
    }
}

/// Bundled syntect syntaxes + a dark theme, loaded once.
struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

fn highlighter() -> &'static Highlighter {
    static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();
    HIGHLIGHTER.get_or_init(|| {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let mut themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .remove("base16-ocean.dark")
            .or_else(|| themes.themes.values().next().cloned())
            .expect("syntect ships at least one default theme");
        Highlighter { syntaxes, theme }
    })
}

/// Highlight a fenced code block into styled, indented lines. Falls back to a
/// flat style when the language is unknown or highlighting fails.
fn highlight_code(lang: &str, code: &str) -> Vec<Line<'static>> {
    const GUTTER: &str = "  ";
    let code = code.trim_end_matches('\n');
    let hl = highlighter();

    let syntax = (!lang.is_empty())
        .then(|| hl.syntaxes.find_syntax_by_token(lang))
        .flatten();
    let Some(syntax) = syntax else {
        // Unknown / no language: render plainly but still distinct.
        return code
            .split('\n')
            .map(|line| {
                Line::from(Span::styled(
                    format!("{GUTTER}{line}"),
                    Style::default().fg(Color::Cyan),
                ))
            })
            .collect();
    };

    let mut highlighter = HighlightLines::new(syntax, &hl.theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, &hl.syntaxes) {
            Ok(ranges) => {
                let mut spans =
                    vec![Span::styled(GUTTER, Style::default().fg(Color::DarkGray))];
                for (style, text) in ranges {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        continue;
                    }
                    let c = style.foreground;
                    spans.push(Span::styled(
                        text.to_string(),
                        Style::default().fg(Color::Rgb(c.r, c.g, c.b)),
                    ));
                }
                lines.push(Line::from(spans));
            }
            Err(_) => lines.push(Line::from(Span::styled(
                format!("{GUTTER}{}", line.trim_end_matches('\n')),
                Style::default().fg(Color::Cyan),
            ))),
        }
    }
    lines
}

fn heading_color(level: HeadingLevel) -> Color {
    match level {
        HeadingLevel::H1 => Color::Cyan,
        HeadingLevel::H2 => Color::Green,
        HeadingLevel::H3 => Color::Yellow,
        _ => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenate all span text of a rendered line for assertions.
    fn line_text(text: &Text, i: usize) -> String {
        text.lines[i]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn heading_is_bold_and_colored() {
        let text = render("# Title");
        let span = &text.lines[0].spans[0];
        assert_eq!(span.content.as_ref(), "Title");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn inline_code_and_emphasis_styled() {
        let text = render("a `code` and *em* word");
        let styles: Vec<_> = text.lines[0].spans.iter().collect();
        let code = styles
            .iter()
            .find(|s| s.content.as_ref() == "code")
            .unwrap();
        assert_eq!(code.style.fg, Some(Color::LightRed));
        let em = styles.iter().find(|s| s.content.as_ref() == "em").unwrap();
        assert!(em.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn fenced_code_block_lines_preserved() {
        let text = render("```rust\nlet x = 1;\nlet y = 2;\n```");
        let joined: String = (0..text.lines.len())
            .map(|i| line_text(&text, i))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("let x = 1;"));
        assert!(joined.contains("let y = 2;"));
        assert!(joined.contains("rust")); // language label
    }

    #[test]
    fn code_block_is_syntax_highlighted() {
        let text = render("```rust\nlet x = 1;\n```");
        let code_line = text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("let")))
            .expect("code content line");
        // syntect emits truecolor (Rgb) fg; the plain fallback would use Cyan.
        let has_rgb = code_line
            .spans
            .iter()
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(..))));
        assert!(has_rgb, "expected syntect Rgb colors in the code line");
        // Distinct tokens (keyword/number/punctuation) get distinct colors.
        let colors: std::collections::HashSet<_> =
            code_line.spans.iter().filter_map(|s| s.style.fg).collect();
        assert!(colors.len() > 1, "expected multiple token colors");
    }

    #[test]
    fn unknown_language_falls_back_without_panic() {
        let text = render("```nonexistentlang\nsome text\n```");
        let joined: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("some text"));
    }

    #[test]
    fn list_items_get_markers() {
        let text = render("- one\n- two");
        let joined: String = (0..text.lines.len())
            .map(|i| line_text(&text, i))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("• one"));
        assert!(joined.contains("• two"));
    }
}
