use std::io::Read as _;
use std::path::Path;
use std::process::ExitCode;

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

const USAGE: &str = "usage: comment_strip_verify --kept-classes <tsv> <orig-root> <new-root>";

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Open(Delimiter),
    Close(Delimiter),
    Punct(char, bool),
    Word(String),
}

struct Proof {
    clap_derives: Vec<String>,
    forwarding_macros: Vec<String>,
}

impl Proof {
    fn read(kept_classes: &str) -> Result<Self, String> {
        Ok(Self {
            clap_derives: names_of_kind(kept_classes, "clap-derive")?,
            forwarding_macros: names_of_kind(kept_classes, "macro-doc")?,
        })
    }
}

fn names_of_kind(kept_classes: &str, kind: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for line in kept_classes.lines() {
        let columns: Vec<&str> = line.split('\t').collect();
        if line.starts_with('#') || columns.get(1) != Some(&kind) {
            continue;
        }
        let pattern = columns.get(3).copied().unwrap_or_default();
        let inner = pattern
            .strip_prefix("^(")
            .and_then(|rest| rest.strip_suffix(")$"))
            .ok_or_else(|| format!("{kind} pattern `{pattern}` is not `^(A|B)$`"))?;
        for name in inner.split('|') {
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(format!("{kind} name `{name}` is not an identifier"));
            }
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn is_punct(tree: Option<&TokenTree>, ch: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

fn attribute_body(tree: Option<&TokenTree>) -> Option<TokenStream> {
    match tree {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => Some(g.stream()),
        _ => None,
    }
}

fn is_doc(body: &TokenStream) -> bool {
    let mut trees = body.clone().into_iter();
    matches!(trees.next(), Some(TokenTree::Ident(i)) if i == "doc")
        && is_punct(trees.next().as_ref(), '=')
}

fn holds_word(stream: &TokenStream, accept: &dyn Fn(&str) -> bool) -> bool {
    stream.clone().into_iter().any(|tree| match tree {
        TokenTree::Ident(i) => accept(&i.to_string()),
        TokenTree::Group(g) => holds_word(&g.stream(), accept),
        _ => false,
    })
}

impl Proof {
    fn derives_clap(&self, body: &TokenStream) -> bool {
        holds_word(body, &|w| w == "derive")
            && holds_word(body, &|w| self.clap_derives.iter().any(|n| n == w))
    }

    fn tokens(&self, stream: TokenStream) -> Vec<Token> {
        let mut out = Vec::new();
        self.flatten(stream, false, &mut out);
        out
    }

    fn flatten(&self, stream: TokenStream, keep_docs: bool, out: &mut Vec<Token>) {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        let mut i = 0;
        while i < trees.len() {
            let inner = is_punct(trees.get(i), '#') && is_punct(trees.get(i + 1), '!');
            let body_at = if inner { i + 2 } else { i + 1 };
            let body = attribute_body(trees.get(body_at)).filter(|_| is_punct(trees.get(i), '#'));
            let Some(body) = body else {
                let reads_docs = keep_docs || self.is_doc_reading_input(&trees, i);
                self.push(&trees[i], reads_docs, out);
                i += 1;
                continue;
            };
            if !keep_docs && !inner && self.run_derives_clap(&trees[i..]) {
                i = self.push_item(&trees, i, out);
            } else if !keep_docs && is_doc(&body) {
                i = body_at + 1;
            } else {
                for tree in &trees[i..=body_at] {
                    self.push(tree, keep_docs, out);
                }
                i = body_at + 1;
            }
        }
    }

    fn is_doc_reading_input(&self, trees: &[TokenTree], i: usize) -> bool {
        if !matches!(trees[i], TokenTree::Group(_)) {
            return false;
        }
        let before = |back: usize| i.checked_sub(back).and_then(|at| trees.get(at));
        let name = match (before(1), before(2)) {
            (bang, name) if is_punct(bang, '!') => name,
            (Some(TokenTree::Ident(_)), bang) if is_punct(bang, '!') => before(3),
            _ => None,
        };
        match name {
            Some(TokenTree::Ident(name)) => !self.forwarding_macros.iter().any(|m| name == m),
            _ => false,
        }
    }

    fn run_derives_clap(&self, trees: &[TokenTree]) -> bool {
        let mut i = 0;
        while is_punct(trees.get(i), '#') {
            match attribute_body(trees.get(i + 1)) {
                Some(body) if self.derives_clap(&body) => return true,
                Some(_) => i += 2,
                None => return false,
            }
        }
        false
    }

    fn push_item(&self, trees: &[TokenTree], mut i: usize, out: &mut Vec<Token>) -> usize {
        let mut angles = 0usize;
        while i < trees.len() {
            self.push(&trees[i], true, out);
            let arrow = i > 0 && is_punct(trees.get(i - 1), '-');
            let ends = match &trees[i] {
                TokenTree::Group(g) => g.delimiter() == Delimiter::Brace && angles == 0,
                TokenTree::Punct(p) if p.as_char() == '<' => {
                    angles += 1;
                    false
                }
                TokenTree::Punct(p) if p.as_char() == '>' && !arrow => {
                    angles = angles.saturating_sub(1);
                    false
                }
                TokenTree::Punct(p) => p.as_char() == ';',
                _ => false,
            };
            i += 1;
            if ends {
                break;
            }
        }
        i
    }

    fn push(&self, tree: &TokenTree, keep_docs: bool, out: &mut Vec<Token>) {
        match tree {
            TokenTree::Group(g) => {
                out.push(Token::Open(g.delimiter()));
                self.flatten(g.stream(), keep_docs, out);
                out.push(Token::Close(g.delimiter()));
            }
            TokenTree::Punct(p) => {
                out.push(Token::Punct(p.as_char(), p.spacing() == Spacing::Joint))
            }
            TokenTree::Ident(i) => out.push(Token::Word(i.to_string())),
            TokenTree::Literal(l) => out.push(Token::Word(l.to_string())),
        }
    }

    fn compare(&self, before: &str, after: &str) -> Option<String> {
        let lexed = |text: &str| without_shebang(text).parse::<TokenStream>();
        let (Ok(a), Ok(b)) = (lexed(before), lexed(after)) else {
            return Some("one version does not lex".to_string());
        };
        if syn::parse_file(before).is_ok() != syn::parse_file(after).is_ok() {
            return Some("only one version parses".to_string());
        }
        let (a, b) = (self.tokens(a), self.tokens(b));
        match a.iter().zip(&b).position(|(x, y)| x != y) {
            Some(at) => Some(format!("token {at}: {:?} became {:?}", a[at], b[at])),
            None if a.len() != b.len() => Some(format!("{} tokens became {}", a.len(), b.len())),
            None => None,
        }
    }
}

fn without_shebang(text: &str) -> &str {
    if text.starts_with("#!") && !text.starts_with("#![") {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        text
    }
}

fn run() -> Result<bool, String> {
    let (mut kept_classes, mut roots) = (None, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kept-classes" => kept_classes = args.next(),
            _ => roots.push(arg),
        }
    }
    let (Some(kept_classes), [orig_root, new_root]) = (kept_classes, roots.as_slice()) else {
        return Err(USAGE.to_string());
    };
    let read =
        |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    let proof = Proof::read(&read(Path::new(&kept_classes))?)?;
    let mut paths = String::new();
    std::io::stdin()
        .read_to_string(&mut paths)
        .map_err(|e| e.to_string())?;
    let mut clean = true;
    for rel in paths.lines().filter(|line| !line.is_empty()) {
        let before = read(&Path::new(orig_root).join(rel))?;
        let after = read(&Path::new(new_root).join(rel))?;
        if let Some(why) = proof.compare(&before, &after) {
            clean = false;
            println!("DIFF\t{rel}\t{why}");
        }
    }
    Ok(clean)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("comment_strip_verify: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept_classes() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/comment-strip/kept-classes.tsv"
        );
        std::fs::read_to_string(path).expect("the kept-class file is in the tree")
    }

    fn proof() -> Proof {
        Proof::read(&kept_classes()).expect("the file's rows parse")
    }

    fn compare(before: &str, after: &str) -> Option<String> {
        proof().compare(before, after)
    }

    #[test]
    fn a_comment_only_change_is_equal() {
        let before = "//! Module.\n/// Doc.\npub fn f() { /* a */ g(); } // b\n";
        assert_eq!(compare(before, "pub fn f() { g(); }\n"), None);
    }

    #[test]
    fn a_changed_token_differs() {
        assert!(compare("fn f() { g(1); }", "fn f() { g(2); }").is_some());
    }

    #[test]
    fn joined_punctuation_differs() {
        let before = "fn f(a: bool, b: bool) -> bool { a &/* c */& b }";
        assert!(compare(before, "fn f(a: bool, b: bool) -> bool { a && b }").is_some());
        assert_eq!(
            compare(before, "fn f(a: bool, b: bool) -> bool { a & & b }"),
            None
        );
    }

    #[test]
    fn a_comment_marker_inside_a_string_is_code() {
        let before = "const A: &str = \"// kept\"; // gone\n";
        assert_eq!(compare(before, "const A: &str = \"// kept\";\n"), None);
        assert!(compare(before, "const A: &str = \"\";\n").is_some());
    }

    #[test]
    fn a_doc_attribute_written_out_is_no_comment() {
        let before = "#[doc(hidden)]\n#[allow(dead_code)]\nfn f() {}\n";
        assert!(compare(before, "#[allow(dead_code)]\nfn f() {}\n").is_some());
    }

    const CLI: &str = "/// The tool.\n#[derive(Parser)]\nstruct Cli {\n    /// Path to read.\n    #[arg(long)]\n    path: String,\n}\n/// Not help.\nstruct Other;\n";

    #[test]
    fn help_text_on_a_clap_item_is_compared() {
        let kept = CLI.replace("/// Not help.\n", "");
        assert_eq!(compare(CLI, &kept), None);
        assert!(compare(CLI, &kept.replace("    /// Path to read.\n", "")).is_some());
        assert!(compare(CLI, &kept.replace("/// The tool.\n", "")).is_some());
        assert!(compare(CLI, &kept.replace("The tool", "INVARIANT: The tool")).is_some());
    }

    #[test]
    fn a_const_generic_default_does_not_end_a_clap_item() {
        let before = "#[derive(Args)]\nstruct Cli<const N: usize = { 3 }> {\n    /// Path.\n    path: String,\n}\n";
        let after = "#[derive(Args)]\nstruct Cli<const N: usize = { 3 }> {\n    path: String,\n}\n";
        assert!(compare(before, after).is_some());
    }

    #[test]
    fn every_clap_derive_in_the_kept_class_file_is_read() {
        let names = names_of_kind(&kept_classes(), "clap-derive").expect("the rows parse");
        assert!(!names.is_empty());
        for name in names {
            let before = format!("#[derive({name})]\nenum E {{\n    /// Help.\n    A,\n}}\n");
            assert!(compare(&before, &before.replace("    /// Help.\n", "")).is_some());
        }
    }

    #[test]
    fn a_pattern_this_side_cannot_read_is_an_error() {
        assert!(names_of_kind("clap-help\tclap-derive\trust\tPars.*\twhy", "clap-derive").is_err());
    }

    #[test]
    fn a_doc_comment_inside_macro_input_is_compared() {
        let before = "tools! {\n    /// Reads a file.\n    read\n}\n";
        assert!(compare(before, "tools! {\n    read\n}\n").is_some());
        assert_eq!(compare(before, before), None);
        let named = "macro_rules! m { () => {\n    /// Doc.\n    fn f() {}\n} }\n";
        assert_eq!(
            compare(named, "macro_rules! m { () => {\n    fn f() {}\n} }\n"),
            None
        );
        let local = "thread_local! {\n    /// Doc.\n    static A: u8 = 0;\n}\n";
        assert_eq!(
            compare(local, "thread_local! {\n    static A: u8 = 0;\n}\n"),
            None
        );
    }

    #[test]
    fn a_trailing_comma_is_a_token() {
        assert!(compare("fn f() { g(1, 2,); }", "fn f() { g(1, 2); }").is_some());
        assert!(compare("fn f() { let _x = (1,); }", "fn f() { let _x = (1); }").is_some());
    }

    #[test]
    fn a_file_that_stops_parsing_differs() {
        assert!(compare("fn f() {}", "fn f() {").is_some());
        assert!(compare("fn f() {}\nstruct A;", "fn f() {}\nstruct;").is_some());
    }

    #[test]
    fn a_shebang_line_lexes() {
        let before = "#!/usr/bin/env run\n// gone\nfn main() {}\n";
        assert_eq!(compare(before, "#!/usr/bin/env run\nfn main() {}\n"), None);
    }
}
