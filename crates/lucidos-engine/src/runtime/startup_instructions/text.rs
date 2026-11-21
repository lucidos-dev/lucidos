//! The text half of Claude Code's startup loading: what it does to one
//! instruction file before it records the file.
//!
//! Claude Code removes block-level HTML comments, drops YAML frontmatter, finds
//! `@path` imports in prose (never in code), and trims the result. The oracle
//! for every rule here is the `instructions` attachment in Claude Code's own
//! transcript, which the ledger compares against (`ledger::StartupCheck`).

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// One file after Claude Code's processing.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Loaded {
    /// The text Claude Code records. Empty means it records nothing.
    pub content: String,
    /// The `@path` imports in the text, in order of appearance, as written.
    pub imports: Vec<String>,
    /// Whether frontmatter gives the file a non-empty `paths` list. A rule with
    /// one loads only when a matching file is read, so it is not a startup file.
    pub path_scoped: bool,
}

pub(super) fn load(raw: &str) -> Loaded {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let (frontmatter, body) = split_frontmatter(raw);
    let (text, imports) = strip_comments_and_find_imports(body);
    Loaded {
        content: trim(&text).to_string(),
        imports,
        path_scoped: frontmatter.is_some_and(declares_paths),
    }
}

/// JavaScript's `trim`, which also removes a byte-order mark.
fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Split a leading `---` block from the rest. A file whose first line is `---`
/// but never closes it has no frontmatter.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, text);
    };
    if first.trim_end() != "---" {
        return (None, text);
    }
    let mut offset = first.len();
    for line in lines {
        if line.trim_end() == "---" {
            return (
                Some(&text[first.len()..offset]),
                &text[offset + line.len()..],
            );
        }
        offset += line.len();
    }
    (None, text)
}

/// Whether the frontmatter has a `paths` key with a value: inline text, or a
/// list on the lines below. A bare `paths:` is null and scopes nothing.
fn declares_paths(frontmatter: &str) -> bool {
    let mut lines = frontmatter.lines();
    while let Some(line) = lines.next() {
        let Some(value) = line.strip_prefix("paths:") else {
            continue;
        };
        if !value.trim().is_empty() {
            return true;
        }
        return lines
            .next()
            .is_some_and(|next| next.trim_start().starts_with('-'));
    }
    false
}

/// One pass over the Markdown: cut the comments and collect the imports.
fn strip_comments_and_find_imports(body: &str) -> (String, Vec<String>) {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut imports = Vec::new();
    let mut prose = String::new();
    let mut in_code_block = false;
    let mut cuts: Vec<(std::ops::Range<usize>, String)> = Vec::new();

    for (event, range) in Parser::new_ext(body, options).into_offset_iter() {
        match &event {
            Event::Text(text) if !in_code_block => prose.push_str(text),
            Event::SoftBreak | Event::HardBreak if !in_code_block => prose.push('\n'),
            _ => end_prose(&mut prose, &mut imports),
        }
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::HtmlBlock) => {
                if let Some(replacement) = without_comment(&body[range.clone()]) {
                    cuts.push((range, replacement));
                }
            }
            _ => {}
        }
    }
    end_prose(&mut prose, &mut imports);

    let mut text = String::with_capacity(body.len());
    let mut copied = 0;
    for (range, replacement) in cuts {
        text.push_str(&body[copied..range.start]);
        text.push_str(&replacement);
        copied = range.end;
    }
    text.push_str(&body[copied..]);
    (text, imports)
}

fn end_prose(prose: &mut String, imports: &mut Vec<String>) {
    if !prose.is_empty() {
        imports.extend(imports_in(prose));
        prose.clear();
    }
}

/// An HTML block that opens with a comment, minus that comment. `None` leaves
/// the block alone: it holds no comment, or the comment never closes. A block
/// with only whitespace left after the comment goes whole, newline included.
fn without_comment(block: &str) -> Option<String> {
    let trimmed = block.trim();
    let after_open = trimmed.strip_prefix("<!--")?;
    let close = after_open.find("-->")?;
    let residue = &after_open[close + "-->".len()..];
    if residue.trim().is_empty() {
        return Some(String::new());
    }
    let open_at = block.find("<!--")?;
    let close_at = open_at + "<!--".len() + block[open_at + "<!--".len()..].find("-->")?;
    Some(format!(
        "{}{}",
        &block[..open_at],
        &block[close_at + "-->".len()..]
    ))
}

/// The `@path` tokens in one run of prose. An import follows whitespace or
/// starts the run, and ends at the next whitespace. A backslash escapes a
/// space. A `#fragment` is not part of the path.
fn imports_in(prose: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = prose.chars().peekable();
    let mut after_space = true;
    while let Some(c) = chars.next() {
        if c != '@' || !after_space {
            after_space = c.is_whitespace();
            continue;
        }
        let mut path = String::new();
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() {
                break;
            }
            if next == '\\' {
                chars.next();
                if chars.peek() == Some(&' ') {
                    chars.next();
                    path.push(' ');
                    continue;
                }
                break;
            }
            path.push(next);
            chars.next();
        }
        let path = path.split('#').next().unwrap_or_default();
        if !path.is_empty() {
            found.push(path.to_string());
        }
        after_space = false;
    }
    found
}
