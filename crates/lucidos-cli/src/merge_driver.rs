//! `lucidos merge-driver`: git merge drivers for files that a text merge gets
//! wrong. A `.gitattributes` line `merge=lucidos-<kind>` picks one, and
//! `scripts/merge-drivers.sh` registers them.
//!
//! The contract with the registered command: write the result over the ours
//! file, then exit 0 for a clean merge or 1 for a conflict. Any other exit
//! means the driver did not merge. The ours file is then untouched, and the
//! registered command merges the path without the driver: as plain text, or
//! by union for the ADR index (`scripts/lib/merge_drivers.sh`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Kind {
    /// One entry per line, keyed by its first number, kept sorted.
    SortedLines,
    /// Markdown headings and top-level bullets, keyed by heading path.
    Blocks,
}

/// What a merge produced.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Merge {
    Clean(Vec<u8>),
    Conflicted(Vec<u8>),
    /// The driver cannot key this file, so git's text merge decides.
    Unkeyable(String),
}

/// Exit status meaning "the driver did not merge; merge without it".
const NOT_MERGED: u8 = 2;

pub(crate) fn run(kind: Kind, base: &Path, ours: &Path, theirs: &Path, marker_size: usize) -> u8 {
    let read = |p: &Path| fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()));
    let inputs = read(base).and_then(|b| Ok((b, read(ours)?, read(theirs)?)));
    let (base_bytes, ours_bytes, theirs_bytes) = match inputs {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lucidos merge-driver: {e}");
            return NOT_MERGED;
        }
    };
    let (bytes, status) = match merge(kind, &base_bytes, &ours_bytes, &theirs_bytes, marker_size) {
        Merge::Clean(out) => (out, 0),
        Merge::Conflicted(out) => (out, 1),
        Merge::Unkeyable(why) => {
            eprintln!("lucidos merge-driver: {why}; merging without the driver");
            return NOT_MERGED;
        }
    };
    match replace_file(ours, &bytes) {
        Ok(()) => status,
        Err(e) => {
            eprintln!("lucidos merge-driver: cannot write {}: {e}", ours.display());
            NOT_MERGED
        }
    }
}

/// Write beside the target, then rename over it. A failure leaves the ours
/// file exactly as git wrote it, which the plain-text merge then reads.
fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".lucidos-merge");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

pub(crate) fn merge(
    kind: Kind,
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    marker_size: usize,
) -> Merge {
    let markers = Markers::new(marker_size);
    let (Ok(base), Ok(ours), Ok(theirs)) = (
        std::str::from_utf8(base),
        std::str::from_utf8(ours),
        std::str::from_utf8(theirs),
    ) else {
        return Merge::Unkeyable("not UTF-8 text".into());
    };
    let parse = match kind {
        Kind::SortedLines => parse_sorted_lines,
        Kind::Blocks => parse_blocks,
    };
    let docs = parse(base).and_then(|b| Ok((b, parse(ours)?, parse(theirs)?)));
    let (base, ours, theirs) = match docs {
        Ok(d) => d,
        Err(why) => return Merge::Unkeyable(why),
    };
    if let Some(why) = renamed_on_both_sides(&base, &ours, &theirs) {
        return Merge::Unkeyable(why);
    }
    let mut slots = three_way(&base, &ours, &theirs, marker_size);
    let order = match kind {
        Kind::SortedLines => sorted_order(&slots),
        Kind::Blocks => match anchored_order(&base, &ours, &theirs, &slots) {
            Ok(order) => order,
            Err(why) => return Merge::Unkeyable(why),
        },
    };
    if kind == Kind::Blocks {
        respace(&order, &mut slots, [&base, &ours, &theirs]);
    }
    render(&order, &slots, &markers)
}

struct Markers {
    open: String,
    mid: String,
    close: String,
}

impl Markers {
    fn new(size: usize) -> Self {
        Markers {
            open: "<".repeat(size),
            mid: "=".repeat(size),
            close: ">".repeat(size),
        }
    }
}

/// A file split into keyed entries, in file order.
#[derive(Debug, Default)]
pub(crate) struct Doc {
    entries: Vec<(String, String)>,
}

impl Doc {
    fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    fn map(&self) -> HashMap<&str, &str> {
        self.entries
            .iter()
            .map(|(k, t)| (k.as_str(), t.as_str()))
            .collect()
    }

    fn push(&mut self, key: String, text: String) -> Result<(), String> {
        if self.entries.iter().any(|(k, _)| *k == key) {
            return Err(format!(
                "two entries share the key {}",
                key.replace(KEY_SEP, " > ")
            ));
        }
        self.entries.push((key, text));
        Ok(())
    }
}

/// Separates the parts of a heading path inside a key.
const KEY_SEP: char = '\u{1f}';
/// The text before the first entry.
const PREAMBLE: &str = "\u{0}preamble";

/// The ADR index shape: a preamble, then only `- ` lines, each carrying a
/// number. The key is the first number, so two branches adding different
/// numbers never collide, and one number added twice is a conflict.
pub(crate) fn parse_sorted_lines(text: &str) -> Result<Doc, String> {
    let mut doc = Doc::default();
    let mut preamble = String::new();
    let mut in_entries = false;
    for line in text.split_inclusive('\n') {
        if !in_entries && !line.starts_with("- ") {
            preamble.push_str(line);
            continue;
        }
        in_entries = true;
        let number = line
            .strip_prefix("- ")
            .and_then(first_number)
            .ok_or_else(|| format!("line {:?} is not a numbered `- ` entry", line.trim_end()))?;
        doc.push(number.to_string(), line.to_string())?;
    }
    doc.entries.insert(0, (PREAMBLE.to_string(), preamble));
    Ok(doc)
}

fn first_number(s: &str) -> Option<u64> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let digits: String = s[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Markdown split at every heading and every top-level bullet, outside code
/// fences. A heading's key is its heading path, and a bullet's is that path
/// plus its bold label or first line. Blank lines before a block belong to
/// it, so an entry added at the end of a list brings its own separator.
///
/// An unindented paragraph after a list is a block of its own, keyed like a
/// bullet. Inside the last bullet, it would move into every bullet appended
/// after it, and two such appends would write it twice.
pub(crate) fn parse_blocks(text: &str) -> Result<Doc, String> {
    let mut doc = Doc::default();
    let mut key = PREAMBLE.to_string();
    let mut block = String::new();
    let mut blanks = String::new();
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut fence: Option<&str> = None;
    let mut lines = text.split_inclusive('\n').peekable();

    if lines.peek().is_some_and(|l| l.trim_end() == "---") {
        block.extend(lines.next());
        for line in lines.by_ref() {
            block.push_str(line);
            if line.trim_end() == "---" {
                break;
            }
        }
    }

    for line in lines {
        if let Some(marker) = fence {
            block.push_str(&std::mem::take(&mut blanks));
            block.push_str(line);
            if line.trim_start().starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if line.trim().is_empty() {
            blanks.push_str(line);
            continue;
        }
        let start = if let Some(level) = heading_level(line) {
            headings.retain(|(l, _)| *l < level);
            headings.push((level, line.trim_end().to_string()));
            Some(format!("h:{}", heading_path(&headings)))
        } else if let Some(label) = bullet_label(line) {
            Some(format!("b:{}{KEY_SEP}{label}", heading_path(&headings)))
        } else if key.starts_with("b:")
            && !blanks.is_empty()
            && !line.starts_with(char::is_whitespace)
        {
            Some(format!(
                "p:{}{KEY_SEP}{}",
                heading_path(&headings),
                line.trim_end()
            ))
        } else {
            None
        };
        match start {
            Some(next) => {
                doc.push(
                    std::mem::replace(&mut key, next),
                    std::mem::take(&mut block),
                )?;
                block = std::mem::take(&mut blanks);
            }
            None => block.push_str(&std::mem::take(&mut blanks)),
        }
        block.push_str(line);
        let trimmed = line.trim_start();
        fence = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
    }
    block.push_str(&blanks);
    doc.push(key, block)?;
    Ok(doc)
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    let rest = &line[hashes..];
    ((1..=6).contains(&hashes) && rest.starts_with(' ')).then_some(hashes)
}

fn heading_path(headings: &[(usize, String)]) -> String {
    let parts: Vec<&str> = headings.iter().map(|(_, h)| h.as_str()).collect();
    parts.join(&KEY_SEP.to_string())
}

fn bullet_label(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))?;
    let label = rest
        .strip_prefix("**")
        .and_then(|bold| bold.split_once("**"))
        .map_or(rest.trim_end(), |(inner, _)| inner);
    Some(label.to_string())
}

/// A key is an entry's first line, so editing that line is a delete plus an
/// add. An entry gone from both sides may be renamed twice. It may be renamed
/// on one side, directly or through its heading, and deleted on the other. It
/// may be trailing text, such as a closing `---`, that each side carried past
/// its own appended entry. Keying would keep both, so the file goes to the
/// merge without the driver.
fn renamed_on_both_sides(base: &Doc, ours: &Doc, theirs: &Doc) -> Option<String> {
    let (b, o, t) = (base.map(), ours.map(), theirs.map());
    let added = ours.keys().chain(theirs.keys()).any(|k| !b.contains_key(k));
    let gone = base
        .keys()
        .find(|k| *k != PREAMBLE && !o.contains_key(k) && !t.contains_key(k))?;
    added.then(|| {
        format!(
            "both sides removed or renamed {}",
            gone.replace(KEY_SEP, " > ")
        )
    })
}

/// One key's merged state.
#[derive(Debug, PartialEq, Eq)]
enum Slot {
    Clean(String),
    /// Git's line merge of one entry, conflict markers included.
    Marked(String),
    /// `None` is a side that deleted the entry.
    Conflict {
        ours: Option<String>,
        theirs: Option<String>,
    },
}

/// The standard three-way rule, per key. A deletion is a change, so a key
/// deleted on one side and edited on the other is a conflict. An entry both
/// sides edited gets git's line merge, so edits to different lines still
/// merge. Leading blank lines move with an entry's position, so they never
/// count as an edit.
fn three_way(base: &Doc, ours: &Doc, theirs: &Doc, marker_size: usize) -> HashMap<String, Slot> {
    let (b, o, t) = (base.map(), ours.map(), theirs.map());
    let keys: HashSet<&str> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut slots = HashMap::new();
    for key in keys {
        let (bv, ov, tv) = (
            b.get(key).copied(),
            o.get(key).copied(),
            t.get(key).copied(),
        );
        let (bc, oc, tc) = (
            bv.map(without_leading_blank_lines),
            ov.map(without_leading_blank_lines),
            tv.map(without_leading_blank_lines),
        );
        let pick = if oc == tc || tc == bc {
            ov.map(|v| Slot::Clean(v.to_string()))
        } else if oc == bc {
            tv.map(|v| Slot::Clean(v.to_string()))
        } else {
            let merged = match (ov, tv) {
                (Some(o), Some(t)) if !(is_one_line(o) && is_one_line(t)) => {
                    line_merge(bv.unwrap_or(""), o, t, marker_size)
                }
                _ => None,
            };
            Some(match merged {
                Some(LineMerge::Clean(text)) => Slot::Clean(text),
                Some(LineMerge::Conflicted(text)) => Slot::Marked(text),
                None => Slot::Conflict {
                    ours: ov.map(|v| v.to_string()),
                    theirs: tv.map(|v| v.to_string()),
                },
            })
        };
        if let Some(slot) = pick {
            slots.insert(key.to_string(), slot);
        }
    }
    slots
}

/// Two different edits of a one-line entry always conflict, so git's line
/// merge has nothing to add there.
fn is_one_line(text: &str) -> bool {
    without_leading_blank_lines(text).trim_end().lines().count() <= 1
}

enum LineMerge {
    Clean(String),
    Conflicted(String),
}

/// `git merge-file` over one entry's three texts. `None` when git could not
/// run, which leaves the whole entry as a conflict.
fn line_merge(base: &str, ours: &str, theirs: &str, marker_size: usize) -> Option<LineMerge> {
    let dir = tempfile::Builder::new()
        .prefix("lucidos-merge-")
        .tempdir()
        .ok()?;
    let paths = [("ours", ours), ("base", base), ("theirs", theirs)].map(|(name, text)| {
        let path = dir.path().join(name);
        fs::write(&path, text).map(|()| path)
    });
    let [Ok(o), Ok(b), Ok(t)] = paths else {
        return None;
    };
    let output = Command::new("git")
        .args([
            "merge-file",
            "-p",
            "-L",
            "ours",
            "-L",
            "base",
            "-L",
            "theirs",
        ])
        .arg(format!("--marker-size={marker_size}"))
        .args([&o, &b, &t])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    match output.status.code()? {
        0 => Some(LineMerge::Clean(text)),
        1..=127 => Some(LineMerge::Conflicted(text)),
        _ => None,
    }
}

fn without_leading_blank_lines(mut text: &str) -> &str {
    while let Some((line, rest)) = text.split_once('\n') {
        if !line.trim().is_empty() {
            break;
        }
        text = rest;
    }
    text
}

fn sorted_order(slots: &HashMap<String, Slot>) -> Vec<String> {
    let mut keys: Vec<&String> = slots.keys().collect();
    keys.sort_by_key(|k| (k.as_str() != PREAMBLE, k.parse::<u64>().unwrap_or(0)));
    keys.into_iter().cloned().collect()
}

/// Order the merged entries. The spine is the side that reordered existing
/// entries, else ours. Each new entry of the other side goes after its
/// nearest placed predecessor. It lands behind the spine's new entries in
/// that section, and never past a heading.
fn anchored_order(
    base: &Doc,
    ours: &Doc,
    theirs: &Doc,
    slots: &HashMap<String, Slot>,
) -> Result<Vec<String>, String> {
    let in_all: HashSet<&str> = {
        let (o, t) = (ours.map(), theirs.map());
        base.keys()
            .filter(|k| o.contains_key(k) && t.contains_key(k))
            .collect()
    };
    let shared = |doc: &Doc| -> Vec<String> {
        doc.keys()
            .filter(|k| in_all.contains(k))
            .map(String::from)
            .collect()
    };
    let (b, o, t) = (shared(base), shared(ours), shared(theirs));
    if o != b && t != b && o != t {
        return Err("both sides reordered existing entries".into());
    }
    let (spine, other) = if o == b && t != b {
        (theirs, ours)
    } else {
        (ours, theirs)
    };

    let base_keys: HashSet<&str> = base.keys().collect();
    let mut order: Vec<String> = spine
        .keys()
        .filter(|k| slots.contains_key(*k))
        .map(String::from)
        .collect();
    let mut from_other: HashSet<String> = HashSet::new();
    let mut previous: Option<&str> = None;
    for key in other.keys() {
        if slots.contains_key(key) && !order.iter().any(|k| k == key) {
            let mut at = previous
                .and_then(|p| order.iter().position(|k| k == p))
                .map_or(0, |i| i + 1);
            while at < order.len()
                && !order[at].starts_with("h:")
                && !base_keys.contains(order[at].as_str())
                && !from_other.contains(&order[at])
            {
                at += 1;
            }
            order.insert(at, key.to_string());
            from_other.insert(key.to_string());
        }
        if order.iter().any(|k| k == key) {
            previous = Some(key);
        }
    }
    check_sections(&order)?;
    Ok(order)
}

/// Every entry must sit under the headings its key names. A merge that would
/// move one under another heading is refused, never written.
fn check_sections(order: &[String]) -> Result<(), String> {
    let mut current: Vec<&str> = Vec::new();
    for key in order {
        let path: Vec<&str> = if let Some(heading) = key.strip_prefix("h:") {
            let path: Vec<&str> = heading.split(KEY_SEP).collect();
            let parent = &path[..path.len() - 1];
            if current.len() < parent.len() || current[..parent.len()] != *parent {
                return Err(format!(
                    "the merge would move {} out of its section",
                    heading.replace(KEY_SEP, " > ")
                ));
            }
            path
        } else if let Some(bullet) = key.strip_prefix("b:").or_else(|| key.strip_prefix("p:")) {
            let (heading, label) = bullet.rsplit_once(KEY_SEP).unwrap_or(("", bullet));
            let path: Vec<&str> = heading.split(KEY_SEP).filter(|p| !p.is_empty()).collect();
            if path != current {
                return Err(format!(
                    "the merge would move the entry {label:?} under another heading"
                ));
            }
            continue;
        } else {
            continue;
        };
        current = path;
    }
    Ok(())
}

/// Leading blank lines belong to an entry's position. A bullet with a new
/// neighbour above takes the spacing its section already uses. So a tight
/// list stays tight and a loose one stays loose. A bullet whose neighbour
/// above is unchanged keeps its text exactly.
fn respace(order: &[String], slots: &mut HashMap<String, Slot>, docs: [&Doc; 3]) {
    let above = |doc: &Doc| -> HashMap<String, String> {
        doc.entries
            .windows(2)
            .map(|w| (w[1].0.clone(), w[0].0.clone()))
            .collect()
    };
    let aboves = docs.map(above);
    // The blank lines a section puts between two bullets, and under its heading.
    let mut between: HashMap<&str, usize> = HashMap::new();
    let mut under: HashMap<&str, usize> = HashMap::new();
    for doc in docs.iter().rev() {
        for w in doc.entries.windows(2) {
            let Some(section) = bullet_section(&w[1].0) else {
                continue;
            };
            let blanks = leading_blank_lines(&w[1].1);
            if bullet_section(&w[0].0) == Some(section) {
                between.insert(section, blanks);
            } else if w[0].0.strip_prefix("h:") == Some(section) {
                under.insert(section, blanks);
            }
        }
    }
    for pair in order.windows(2) {
        let (prev, key) = (&pair[0], &pair[1]);
        let Some(section) = bullet_section(key) else {
            continue;
        };
        let Some(Slot::Clean(text)) = slots.get(key) else {
            continue;
        };
        let kept = docs.iter().zip(&aboves).any(|(doc, above)| {
            doc.entries.iter().any(|(k, t)| k == key && t == text) && above.get(key) == Some(prev)
        });
        if kept {
            continue;
        }
        let blanks = if bullet_section(prev) == Some(section) {
            between.get(section).copied().unwrap_or(0)
        } else if prev.strip_prefix("h:") == Some(section) {
            under.get(section).copied().unwrap_or(1)
        } else {
            continue;
        };
        let respaced = format!(
            "{}{}",
            "\n".repeat(blanks),
            without_leading_blank_lines(text)
        );
        slots.insert(key.clone(), Slot::Clean(respaced));
    }
}

/// The heading path a bullet key names.
fn bullet_section(key: &str) -> Option<&str> {
    let bullet = key.strip_prefix("b:")?;
    Some(
        bullet
            .rsplit_once(KEY_SEP)
            .map_or("", |(heading, _)| heading),
    )
}

fn leading_blank_lines(text: &str) -> usize {
    text.split_inclusive('\n')
        .take_while(|line| line.trim().is_empty() && line.ends_with('\n'))
        .count()
}

fn render(order: &[String], slots: &HashMap<String, Slot>, markers: &Markers) -> Merge {
    let mut out = String::new();
    let mut conflicted = false;
    for key in order {
        // An entry that ended its file may lack a newline. It must not join
        // the line of whatever the merge put after it.
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        match &slots[key] {
            Slot::Clean(text) => out.push_str(text),
            Slot::Marked(text) => {
                conflicted = true;
                out.push_str(text);
            }
            Slot::Conflict { ours, theirs } => {
                conflicted = true;
                out.push_str(&format!("{} ours\n", markers.open));
                push_terminated_str(&mut out, ours.as_deref().unwrap_or(""));
                out.push_str(&format!("{}\n", markers.mid));
                push_terminated_str(&mut out, theirs.as_deref().unwrap_or(""));
                out.push_str(&format!("{} theirs\n", markers.close));
            }
        }
    }
    if conflicted {
        Merge::Conflicted(out.into_bytes())
    } else {
        Merge::Clean(out.into_bytes())
    }
}

fn push_terminated_str(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
}

#[cfg(test)]
#[path = "merge_driver_tests.rs"]
mod tests;
