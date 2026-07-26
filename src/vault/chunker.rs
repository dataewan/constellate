use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::Chunk;

/// Split Markdown into heading-grouped chunks. Content before the first heading
/// becomes a leading chunk with an empty header.
pub fn chunk(markdown: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut header = String::new();
    let mut body = String::new();
    let mut in_heading = false;

    let mut flush = |header: &mut String, body: &mut String| {
        if !header.trim().is_empty() || !body.trim().is_empty() {
            chunks.push(Chunk {
                header: header.trim().to_string(),
                content: body.trim().to_string(),
            });
        }
        header.clear();
        body.clear();
    };

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                flush(&mut header, &mut body);
                in_heading = true;
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
            }
            Event::Text(text) | Event::Code(text) => {
                if in_heading {
                    header.push_str(&text);
                } else {
                    body.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if !in_heading {
                    body.push('\n');
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                if !in_heading {
                    body.push('\n');
                }
            }
            _ => {}
        }
    }

    // Flush the trailing section without borrowing the closure again.
    if !header.trim().is_empty() || !body.trim().is_empty() {
        chunks.push(Chunk {
            header: header.trim().to_string(),
            content: body.trim().to_string(),
        });
    }

    chunks
}

/// Collect Markdown link destinations (`[text](dest)` and reference links).
pub fn markdown_links(markdown: &str) -> Vec<String> {
    let mut links = Vec::new();
    for event in Parser::new(markdown) {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            let dest = dest_url.to_string();
            // Keep intra-vault references; skip web/anchor links.
            if !dest.is_empty() && !dest.starts_with("http") && !dest.starts_with('#') {
                links.push(dest);
            }
        }
    }
    links
}
