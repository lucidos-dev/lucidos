use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Loaded {
    pub content: String,
    pub imports: Vec<String>,
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

fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

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
