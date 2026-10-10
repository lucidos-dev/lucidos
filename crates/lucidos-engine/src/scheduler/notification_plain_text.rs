//! The plain-text form of a notification body, for the surfaces that cannot
//! render markdown: the in-app toast, the macOS banner and the web push.
//!
//! The body is markdown, and the notification detail view renders it as such.
//! An OS banner shows its text verbatim, so every `**` and `- ` the author
//! wrote would reach the lock screen as literal characters.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// `markdown` with its markup removed and its structure kept as line breaks.
///
/// Blocks are separated by a blank line and list items start with a bullet.
/// A link keeps its label and an image keeps its alt text. Raw HTML keeps its
/// text and loses its tags, with `<br>` as a line break.
pub fn plain_text_body(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    // One entry per open list: `Some(n)` is ordered and numbers from n.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::Html(html) | Event::InlineHtml(html) => push_html_text(&mut out, &html),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::TaskListMarker(done) => out.push_str(if done { "[x] " } else { "[ ] " }),
            Event::Start(Tag::List(first)) => lists.push(first),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                // A nested list ends inside its parent's item.
                if lists.is_empty() {
                    end_block(&mut out);
                } else {
                    start_line(&mut out);
                }
            }
            Event::Start(Tag::Item) => {
                start_line(&mut out);
                match lists.last_mut() {
                    Some(Some(n)) => {
                        out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => out.push_str("• "),
                }
            }
            Event::End(TagEnd::TableCell) => out.push_str(" | "),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                let row_len = out.trim_end_matches(" | ").len();
                out.truncate(row_len);
                out.push('\n');
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::BlockQuote(_)
                | TagEnd::HtmlBlock
                | TagEnd::Table,
            ) => end_block(&mut out),
            Event::Rule => {
                start_line(&mut out);
                end_block(&mut out);
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Append the text of an HTML fragment: tags go, and `<br>` becomes a line
/// break. An unclosed `<` is text, as the markdown renderer shows it.
fn push_html_text(out: &mut String, html: &str) {
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            out.push_str(&rest[open..]);
            return;
        };
        let tag = &rest[open + 1..open + close];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if name.eq_ignore_ascii_case("br") {
            out.push('\n');
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
}

/// Leave `out` at the start of a line.
fn start_line(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Close a block with one blank line, never more.
fn end_block(out: &mut String) {
    let len = out.trim_end_matches('\n').len();
    out.truncate(len);
    if !out.is_empty() {
        out.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::plain_text_body;

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(plain_text_body("Pick one"), "Pick one");
        assert_eq!(
            plain_text_body("Ship it? Yes: 3 of 4 passed."),
            "Ship it? Yes: 3 of 4 passed."
        );
    }

    #[test]
    fn a_question_card_body_reads_as_plain_text() {
        let body = "**The fix is live.** All four commits are in main.\n\n\
                    What changed:\n\n\
                    - **Block markdown:** paragraphs keep their structure.\n\
                    - **Weight:** only the last paragraph is bold.\n\n\
                    What should I do with it?";
        assert_eq!(
            plain_text_body(body),
            "The fix is live. All four commits are in main.\n\n\
             What changed:\n\n\
             • Block markdown: paragraphs keep their structure.\n\
             • Weight: only the last paragraph is bold.\n\n\
             What should I do with it?"
        );
    }

    #[test]
    fn ordered_lists_keep_their_numbers() {
        assert_eq!(plain_text_body("3. three\n4. four"), "3. three\n4. four");
    }

    #[test]
    fn links_keep_the_label_and_images_the_alt_text() {
        assert_eq!(
            plain_text_body(
                "See [the PR](https://example.com/pr/1) and ![a chart](artifacts/c.png)."
            ),
            "See the PR and a chart."
        );
    }

    #[test]
    fn headings_code_and_quotes_lose_their_markers() {
        assert_eq!(
            plain_text_body("# Build failed\n\n> run `make test`\n\n```\nexit 101\n```"),
            "Build failed\n\nrun make test\n\nexit 101"
        );
    }

    #[test]
    fn raw_html_loses_its_tags_and_keeps_its_text() {
        assert_eq!(plain_text_body("a <b>bold</b> word"), "a bold word");
        assert_eq!(plain_text_body("text<br>more"), "text\nmore");
    }

    #[test]
    fn an_html_block_keeps_its_text() {
        assert_eq!(
            plain_text_body("<p>Build failed on main</p>\nsee the log"),
            "Build failed on main\nsee the log"
        );
        assert_eq!(
            plain_text_body("<details>\n<summary>Log</summary>\nexit 101\n</details>\n\nDone."),
            "Log\nexit 101\n\nDone."
        );
    }

    #[test]
    fn strikethrough_loses_its_markers() {
        assert_eq!(plain_text_body("~~cancelled~~ moved"), "cancelled moved");
    }

    #[test]
    fn a_nested_list_adds_no_blank_line() {
        assert_eq!(plain_text_body("- a\n  - b\n- c"), "• a\n• b\n• c");
    }

    #[test]
    fn a_table_becomes_one_line_per_row() {
        assert_eq!(
            plain_text_body("| A | B |\n| --- | --- |\n| 1 | 2 |"),
            "A | B\n1 | 2"
        );
    }

    #[test]
    fn a_line_break_inside_a_paragraph_survives() {
        assert_eq!(plain_text_body("line one\nline two"), "line one\nline two");
    }
}
