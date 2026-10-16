//! Write-time lint for an app or widget file. It finds three mistakes that
//! look right in a desktop preview and read wrong on a phone. The file tools
//! append the findings to their result, so the agent fixes them in the same
//! turn. It never refuses or changes a write.
//!
//! The rules are documented for app authors in
//! `system-knowhow/building-an-app.md` § Scaffolding defaults.

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// The most findings one note lists. A file past it gets a count of the rest,
/// so a stylesheet full of ungated hovers cannot flood the context.
const MAX_LISTED_FINDINGS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AppLintRule {
    UngatedHover,
    ActionBtnAsState,
    VariantWithoutBase { variant: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AppLintFinding {
    /// 1-based, in the file as written.
    pub line: usize,
    pub rule: AppLintRule,
}

impl AppLintFinding {
    fn message(&self) -> String {
        let line = self.line;
        match &self.rule {
            AppLintRule::UngatedHover => format!(
                "line {line}: a `:hover` rule outside `@media (hover: hover)` stays on after a \
                 tap on a touch screen, so move it inside `@media (hover: hover) {{ … }}`."
            ),
            AppLintRule::ActionBtnAsState => format!(
                "line {line}: swapping `action-btn` variants shows the current choice as a button \
                 to press, so use `.segmented-btn.active` for one-of-N or `.pill-bar-btn` with \
                 `aria-pressed` for a filter."
            ),
            AppLintRule::VariantWithoutBase { variant } => format!(
                "line {line}: `{variant}` without the base `action-btn` draws a plain browser \
                 button, so write `class=\"action-btn {variant}\"`."
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Html,
    Css,
    Js,
}

/// What a file under `apps/<id>/` holds, by extension. `None` for any other
/// path, and for app files the lint does not read (`manifest.json`, images).
fn app_file_kind(data_path: &str) -> Option<FileKind> {
    let (id, file) = data_path.strip_prefix("apps/")?.split_once('/')?;
    if id.is_empty() || file.is_empty() {
        return None;
    }
    let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => Some(FileKind::Html),
        "css" => Some(FileKind::Css),
        "js" | "mjs" => Some(FileKind::Js),
        _ => None,
    }
}

/// The note a file tool appends to its result, or `None` when the path is no
/// app file or the file is clean.
pub(crate) fn app_lint_note(data_path: &str, content: &str) -> Option<String> {
    let findings = lint_app_file(data_path, content);
    if findings.is_empty() {
        return None;
    }
    let mut note = format!(
        "\n\n[APP LINT] {data_path}: {} problem(s). The file is saved; fix them now:",
        findings.len()
    );
    for finding in findings.iter().take(MAX_LISTED_FINDINGS) {
        note.push_str("\n  ");
        note.push_str(&finding.message());
    }
    if findings.len() > MAX_LISTED_FINDINGS {
        note.push_str(&format!(
            "\n  …and {} more of the same kinds.",
            findings.len() - MAX_LISTED_FINDINGS
        ));
    }
    Some(note)
}

/// The note for a file a tool has just written under `data_dir`, read back
/// from disk. For a tool that does not hold the final content in hand: an edit
/// or a script's output.
pub(crate) fn app_lint_note_on_disk(data_dir: &Path, data_path: &str) -> Option<String> {
    app_file_kind(data_path)?;
    match std::fs::read_to_string(data_dir.join(data_path)) {
        Ok(content) => app_lint_note(data_path, &content),
        Err(e) => Some(format!(
            "\n\n[APP LINT] {data_path}: could not read the file back to lint it: {e}"
        )),
    }
}

pub(crate) fn lint_app_file(data_path: &str, content: &str) -> Vec<AppLintFinding> {
    let Some(kind) = app_file_kind(data_path) else {
        return Vec::new();
    };
    let source = Source::new(kind, content);
    let mut findings = Vec::new();
    for range in &source.css {
        for offset in ungated_hover_offsets(&source.text[range.clone()]) {
            findings.push(source.finding(range.start + offset, AppLintRule::UngatedHover));
        }
    }
    for range in &source.js {
        for offset in action_btn_state_offsets(&source.text[range.clone()]) {
            findings.push(source.finding(range.start + offset, AppLintRule::ActionBtnAsState));
        }
    }
    if kind != FileKind::Css {
        for (offset, variant) in variant_without_base(&source.text) {
            findings.push(source.finding(offset, AppLintRule::VariantWithoutBase { variant }));
        }
    }
    // Line first, then rule, so every duplicate sits beside its twin.
    findings.sort();
    findings.dedup();
    findings
}

/// A file with its comments blanked to spaces, and the byte ranges holding CSS
/// and JS. Blanking keeps every offset and newline where it was, so a match
/// in the blanked text names the right line of the file.
struct Source {
    text: String,
    css: Vec<Range<usize>>,
    js: Vec<Range<usize>>,
    /// The offset of every `\n`, so a finding finds its line by binary search.
    newlines: Vec<usize>,
}

impl Source {
    fn new(kind: FileKind, content: &str) -> Self {
        let mut bytes = content.as_bytes().to_vec();
        let whole = 0..bytes.len();
        let (css, js) = match kind {
            FileKind::Css => {
                blank_css_comments(&mut bytes, whole.clone());
                (vec![whole], Vec::new())
            }
            FileKind::Js => {
                blank_js_comments(&mut bytes, whole.clone());
                (Vec::new(), vec![whole])
            }
            FileKind::Html => {
                blank_html_comments(&mut bytes);
                let markup = String::from_utf8_lossy(&bytes).into_owned();
                let css = element_bodies(&STYLE_ELEMENT, &markup);
                let js = element_bodies(&SCRIPT_ELEMENT, &markup);
                for range in &css {
                    blank_css_comments(&mut bytes, range.clone());
                }
                for range in &js {
                    blank_js_comments(&mut bytes, range.clone());
                }
                (css, js)
            }
        };
        // Blanking replaces whole comments, delimiters included, and every
        // delimiter is ASCII, so no multibyte character is ever split.
        let text = String::from_utf8(bytes).expect("blanking keeps UTF-8 intact");
        let newlines = text.match_indices('\n').map(|(i, _)| i).collect();
        Self {
            text,
            css,
            js,
            newlines,
        }
    }

    fn finding(&self, offset: usize, rule: AppLintRule) -> AppLintFinding {
        let line = self.newlines.partition_point(|&n| n < offset) + 1;
        AppLintFinding { line, rule }
    }
}

static STYLE_ELEMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style\b[^>]*>(.*?)</style\s*>").unwrap());
static SCRIPT_ELEMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>(.*?)</script\s*>").unwrap());

fn element_bodies(element: &Regex, markup: &str) -> Vec<Range<usize>> {
    element
        .captures_iter(markup)
        .filter_map(|c| c.get(1).map(|m| m.range()))
        .collect()
}

fn blank(bytes: &mut [u8], range: Range<usize>) {
    for b in &mut bytes[range] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

fn blank_html_comments(bytes: &mut [u8]) {
    let mut i = 0;
    while let Some(start) = find(bytes, i, b"<!--") {
        let end = find(bytes, start + 4, b"-->").map_or(bytes.len(), |e| e + 3);
        blank(bytes, start..end);
        i = end;
    }
}

/// Blanks `/* … */` comments, stepping over quoted strings so a `/*` inside
/// one stays.
fn blank_css_comments(bytes: &mut [u8], range: Range<usize>) {
    let mut i = range.start;
    while i < range.end {
        match bytes[i] {
            b'"' | b'\'' => i = skip_string(bytes, i, range.end),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = find(&bytes[..range.end], i + 2, b"*/").map_or(range.end, |e| e + 2);
                blank(bytes, i..end);
                i = end;
            }
            _ => i += 1,
        }
    }
}

/// Blanks `// …` and `/* … */` comments, stepping over quoted and template
/// strings so the `//` of a URL in a string stays.
fn blank_js_comments(bytes: &mut [u8], range: Range<usize>) {
    let mut i = range.start;
    while i < range.end {
        match bytes[i] {
            b'"' | b'\'' | b'`' => i = skip_string(bytes, i, range.end),
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = bytes[i..range.end]
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(range.end, |p| i + p);
                blank(bytes, i..end);
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = find(&bytes[..range.end], i + 2, b"*/").map_or(range.end, |e| e + 2);
                blank(bytes, i..end);
                i = end;
            }
            _ => i += 1,
        }
    }
}

/// The index just past the string opening at `start`, honouring backslash
/// escapes. An unterminated string runs to `end`.
fn skip_string(bytes: &[u8], start: usize, end: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < end {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    end
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Offsets of every style rule whose selector holds `:hover` with no enclosing
/// `@media` that requires a hover-capable pointer.
fn ungated_hover_offsets(css: &str) -> Vec<usize> {
    let bytes = css.as_bytes();
    let mut gates: Vec<bool> = Vec::new();
    let mut prelude_start = 0;
    let mut offsets = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                i = skip_string(bytes, i, bytes.len());
                continue;
            }
            b'{' => {
                let prelude = &css[prelude_start..i];
                let header = prelude.trim_start();
                let header_offset = prelude_start + (prelude.len() - header.len());
                let gate = match header.strip_prefix("@media") {
                    Some(condition) => is_hover_gate(condition),
                    None => {
                        let gated = gates.iter().any(|g| *g);
                        if !header.starts_with('@') && header.contains(":hover") && !gated {
                            offsets.push(header_offset);
                        }
                        false
                    }
                };
                gates.push(gate);
                prelude_start = i + 1;
            }
            b'}' => {
                gates.pop();
                prelude_start = i + 1;
            }
            b';' => prelude_start = i + 1,
            _ => {}
        }
        i += 1;
    }
    offsets
}

/// Whether a media condition holds only where the pointer can hover. Matches
/// `(hover: hover)` and `(any-hover: hover)`, alone or joined with `and`.
fn is_hover_gate(condition: &str) -> bool {
    let compact: String = condition
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    // A comma or `or` joins alternatives, and any one of them may match a
    // touch screen. So every alternative has to require hover.
    compact
        .replace(")or(", "),(")
        .split(',')
        .all(|query| query.contains("hover:hover)") && !query.starts_with("not"))
}

/// A JS string literal: single, double or backtick quoted.
const STRING_LITERAL: &str = r#"'(?:[^'\\\n]|\\.)*'|"(?:[^"\\\n]|\\.)*"|`(?:[^`\\]|\\.)*`"#;

static CLASSLIST_SWAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"classList\s*\.\s*(?:toggle|remove|replace)\s*\(([^)]*)").unwrap()
});
static TERNARY_OF_STRINGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\?\s*({STRING_LITERAL})\s*:\s*({STRING_LITERAL})"
    ))
    .unwrap()
});
static CLASS_NAME_ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"className\s*=\s*({STRING_LITERAL})")).unwrap());
static STRING_LITERALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(STRING_LITERAL).unwrap());
static BRANCH_KEYWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:if|else)\b").unwrap());
/// A condition that reads as "is this the current choice". Without it, a
/// ternary picking a variant from data (`b.primary ? … : …`) is a legitimate
/// mix of primary and secondary buttons, not a selection.
static SELECTION_CONDITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)==|active|selected|current|sort|filter|view|mode|tab|picked|chosen|checked|pressed",
    )
    .unwrap()
});

const BASE_CLASS: &str = "action-btn";
const SECONDARY_CLASS: &str = "action-btn-secondary";
const VARIANT_CLASSES: [&str; 3] = [SECONDARY_CLASS, "action-btn-confirm", "action-btn-danger"];

/// The class tokens a string literal or attribute value names.
fn class_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|t| !t.is_empty())
}

fn names_class(text: &str, class: &str) -> bool {
    class_tokens(text).any(|t| t == class)
}

/// Offsets of JS that shows a selected state by swapping `.action-btn`
/// variants: a `classList` toggle, remove or replace of the base or the
/// secondary class, or a selection-reading branch that picks the secondary.
fn action_btn_state_offsets(js: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    for c in CLASSLIST_SWAP.captures_iter(js) {
        let args = c.get(1).map_or("", |m| m.as_str());
        let swaps = STRING_LITERALS.find_iter(args).any(|s| {
            names_class(s.as_str(), BASE_CLASS) || names_class(s.as_str(), SECONDARY_CLASS)
        });
        if swaps {
            offsets.push(c.get(0).map_or(0, |m| m.start()));
        }
    }
    for c in TERNARY_OF_STRINGS.captures_iter(js) {
        let whole = c.get(0).expect("group 0 always matches");
        let picks_secondary = (1..=2).any(|g| {
            c.get(g)
                .is_some_and(|m| names_class(m.as_str(), SECONDARY_CLASS))
        });
        if picks_secondary && SELECTION_CONDITION.is_match(line_before(js, whole.start())) {
            offsets.push(whole.start());
        }
    }
    for c in CLASS_NAME_ASSIGNMENT.captures_iter(js) {
        let whole = c.get(0).expect("group 0 always matches");
        let value = c.get(1).map_or("", |m| m.as_str());
        if !names_class(value, SECONDARY_CLASS) {
            continue;
        }
        let context = lines_before(js, whole.start(), 3);
        if BRANCH_KEYWORD.is_match(context) && SELECTION_CONDITION.is_match(context) {
            offsets.push(whole.start());
        }
    }
    offsets
}

/// The text from the start of `offset`'s line up to `offset`.
fn line_before(text: &str, offset: usize) -> &str {
    let start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    &text[start..offset]
}

/// The text from `count` lines above `offset`'s line up to `offset`.
fn lines_before(text: &str, offset: usize, count: usize) -> &str {
    let mut start = offset;
    for _ in 0..=count {
        match text[..start].rfind('\n') {
            Some(p) => start = p,
            None => return &text[..offset],
        }
    }
    &text[start + 1..offset]
}

static CLASS_ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bclass\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());

/// Offsets of class lists naming an `.action-btn` variant without the base
/// class: a `class="…"` attribute (in markup or a template string) or a
/// `className = "…"` assignment.
fn variant_without_base(text: &str) -> Vec<(usize, String)> {
    let attributes = CLASS_ATTRIBUTE
        .captures_iter(text)
        .filter_map(|c| Some((c.get(0)?.start(), c.get(1).or(c.get(2))?.as_str())));
    let assignments = CLASS_NAME_ASSIGNMENT
        .captures_iter(text)
        .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str())));
    attributes
        .filter(|(offset, _)| !is_attribute_selector(text, *offset))
        .chain(assignments)
        .filter_map(|(offset, value)| {
            if names_class(value, BASE_CLASS) {
                return None;
            }
            let variant = VARIANT_CLASSES.iter().find(|v| names_class(value, v))?;
            Some((offset, (*variant).to_string()))
        })
        .collect()
}

/// Whether the `class=` at `offset` sits inside `[…]`: a selector that matches
/// elements, as in CSS or `querySelector`, rather than a class list it sets.
fn is_attribute_selector(text: &str, offset: usize) -> bool {
    text[..offset].trim_end().ends_with('[')
}

#[cfg(test)]
#[path = "app_lint_tests.rs"]
mod tests;
