/// Rewrite app HTML for historical-version serving.
///
/// When the user requests `?commit=X`, sub-resource requests must hit the
/// same commit so the historical app is internally consistent. We append
/// `?commit=X` to relative `src`/`href` attributes server-side; the browser
/// then issues those requests with the suffix and the engine serves the
/// historical version.
///
/// For current-version requests (`commit = None`), the HTML is returned
/// untouched — apps are pure static content. SDK, theme CSS, audio shim,
/// favicon, `<title>`, and any other conveniences are opt-in via tags the
/// app author includes (see `knowhow/js-sdk.md`).
pub(super) fn rewrite_for_commit(html: &str, commit: Option<&str>) -> String {
    match commit {
        Some(commit_hash) => {
            let suffix = format!("?commit={}", commit_hash);
            append_query_to_relative_paths(html, &suffix)
        }
        None => html.to_string(),
    }
}

/// Append `?thread_id=<id>` to every relative `src` / `href` in the served
/// HTML when previewing an app from an *app coding-agent thread*'s worktree.
/// Without this, sub-resources (CSS, JS, images, fonts) resolve via the
/// route's live-workspace branch — defeating the preview, which is meant to
/// show the whole app from the WIP. Mirrors `rewrite_for_commit`.
pub(super) fn rewrite_for_thread_id(html: &str, thread_id: &str) -> String {
    let suffix = format!("?thread_id={}", thread_id);
    append_query_to_relative_paths(html, &suffix)
}

/// Append a query string (e.g. `?commit=abc123`) to relative src/href attributes in HTML.
/// Only rewrites in markup — skips `<script>` block *bodies* where template literals like
/// `src="${var}"` would be incorrectly rewritten. The opening `<script>` tag's own attributes
/// (e.g. `<script src="script.js">`) ARE rewritten so external scripts resolve correctly.
fn append_query_to_relative_paths(html: &str, suffix: &str) -> String {
    static SCRIPT_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?si)(<script[\s][^>]*>|<script>)(.*?</script>)").unwrap()
    });
    static ATTR_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"((?:src|href)\s*=\s*")([^"/][^"]*)"#).unwrap()
    });

    let mut result = String::with_capacity(html.len());
    let mut last_end = 0;

    for caps in SCRIPT_RE.captures_iter(html) {
        let full = caps.get(0).unwrap();
        let opening_tag = &caps[1];
        let body_and_close = &caps[2];

        let before = &html[last_end..full.start()];
        result.push_str(&append_suffix_to_attrs(before, suffix, &ATTR_RE));
        result.push_str(&append_suffix_to_attrs(opening_tag, suffix, &ATTR_RE));
        result.push_str(body_and_close);
        last_end = full.end();
    }

    let after = &html[last_end..];
    result.push_str(&append_suffix_to_attrs(after, suffix, &ATTR_RE));

    result
}

/// Append a query suffix to relative src/href attributes in an HTML fragment.
fn append_suffix_to_attrs(html: &str, suffix: &str, re: &regex::Regex) -> String {
    re.replace_all(html, |caps: &regex::Captures| {
        let path = &caps[2];
        if path.starts_with("data:")
            || path.starts_with("http:")
            || path.starts_with("https:")
            || path.starts_with("mailto:")
            || path.starts_with("javascript:")
            || path.starts_with('#')
        {
            format!("{}{}", &caps[1], path)
        } else {
            format!("{}{}{}", &caps[1], path, suffix)
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_commit_returns_html_unchanged() {
        let html = "<html><head><title>App</title></head><body>hi</body></html>";
        assert_eq!(rewrite_for_commit(html, None), html);
    }

    #[test]
    fn commit_appended_to_relative_attrs() {
        let html = r#"<link href="style.css"><script src="app.js"></script>"#;
        let result = rewrite_for_commit(html, Some("abc123"));
        assert!(result.contains(r#"href="style.css?commit=abc123""#));
        assert!(result.contains(r#"src="app.js?commit=abc123""#));
    }

    #[test]
    fn commit_not_appended_to_absolute_or_special_urls() {
        let html = r##"<a href="https://example.com">x</a><a href="/api/v1/sdk.js">y</a><a href="#anchor">z</a>"##;
        let result = rewrite_for_commit(html, Some("abc123"));
        assert!(
            !result.contains("?commit"),
            "absolute / anchor / root-path URLs must not be rewritten: {}",
            result
        );
    }

    #[test]
    fn commit_does_not_rewrite_script_body_template_literals() {
        // Template literals like `src="${var}"` inside <script> bodies must not be
        // rewritten — but the opening <script src=""> tag's own attrs are.
        let html =
            r#"<script src="lib.js"></script><script>var x = `<img src="foo.png">`;</script>"#;
        let result = rewrite_for_commit(html, Some("abc"));
        assert!(
            result.contains(r#"src="lib.js?commit=abc""#),
            "opening tag attrs rewritten"
        );
        assert!(
            result.contains(r#"src="foo.png">"#),
            "script body untouched: {}",
            result
        );
        assert!(
            !result.contains(r#"foo.png?commit"#),
            "script body must not be rewritten"
        );
    }
}
