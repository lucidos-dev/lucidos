/// One `<script>` block: the opening tag, then its body up to `</script>`.
/// Both HTML rewrites below split on this, so it is defined once.
static SCRIPT_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?si)(<script[\s][^>]*>|<script>)(.*?</script>)")
        .expect("app html script-block regex must compile")
});

/// Apply `rewrite` to every part of `html` outside a `<script>` body: the markup
/// between script blocks, and each opening `<script …>` tag's own attributes.
///
/// A script body passes through verbatim, so an inline-JS string literal such as
/// `src="${var}"` is never rewritten. The opening tag still is, so an external
/// `<script src="app.js">` resolves.
fn rewrite_outside_script_bodies(html: &str, rewrite: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len() + 16);
    let mut last_end = 0;
    for caps in SCRIPT_RE.captures_iter(html) {
        let full = caps.get(0).expect("capture group 0 always matches");
        out.push_str(&rewrite(&html[last_end..full.start()]));
        out.push_str(&rewrite(&caps[1]));
        out.push_str(&caps[2]);
        last_end = full.end();
    }
    out.push_str(&rewrite(&html[last_end..]));
    out
}

/// Append `?thread_id=<id>` to every relative `src` / `href` in the served
/// HTML when previewing an app from an *app coding-agent thread*'s worktree.
/// Without this, sub-resources (CSS, JS, images, fonts) resolve via the
/// route's live-workspace branch — defeating the preview, which is meant to
/// show the whole app from the WIP.
pub(super) fn rewrite_for_thread_id(html: &str, thread_id: &str) -> String {
    let suffix = format!("?thread_id={}", thread_id);
    append_query_to_relative_paths(html, &suffix)
}

/// Re-scope an app UI page's **absolute** engine/asset references with the
/// workspace path prefix (ADR 0014 §4). Behind the gateway an app iframe loads
/// at `/<slug>/app/<id>/`, and its absolute refs (`<script src="/api/v1/sdk.js">`
/// — the documented SDK contract) must route back to this workspace's engine as
/// `/<slug>/api/v1/sdk.js`. The gateway no longer rewrites bodies, so the engine
/// does this one adjustment itself from the forwarded prefix.
///
/// A `prefix` of `/` (direct access, no gateway) is a no-op. Only root-absolute
/// refs to engine routes / bundled assets are rewritten; the app's own relative
/// refs (`./style.css`) already resolve against the iframe URL and are left
/// alone, and `<script>` BODIES are skipped so inline-JS string literals aren't
/// corrupted. Idempotent in practice (already-prefixed paths don't re-match).
pub(super) fn rescope_app_html(html: &str, prefix: &str) -> String {
    if prefix == "/" {
        return html.to_string();
    }
    // `prefix` is `/<slug>/`; the rewrite inserts `/<slug>` before the matched
    // root-absolute path (which keeps its own leading `/`).
    let slug = prefix.trim_end_matches('/');

    static ATTR_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)((?:src|href)\s*=\s*")(/(?:api/v1/|app/|data/|assets/|icons/|splash/|favicon|manifest\.json|sw\.js)[^"]*)"#,
        )
        .expect("app rescope attr regex must compile")
    });

    rewrite_outside_script_bodies(html, |fragment| {
        ATTR_RE
            .replace_all(fragment, |caps: &regex::Captures| {
                format!("{}{}{}", &caps[1], slug, &caps[2])
            })
            .into_owned()
    })
}

/// Append a query string (e.g. `?thread_id=abc123`) to relative src/href
/// attributes in HTML. Script bodies are skipped; see
/// [`rewrite_outside_script_bodies`].
fn append_query_to_relative_paths(html: &str, suffix: &str) -> String {
    static ATTR_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"((?:src|href)\s*=\s*")([^"/][^"]*)"#)
            .expect("app thread-id attr regex must compile")
    });

    rewrite_outside_script_bodies(html, |fragment| {
        append_suffix_to_attrs(fragment, suffix, &ATTR_RE)
    })
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
    fn thread_id_appended_to_relative_attrs() {
        let html = r#"<link href="style.css"><script src="app.js"></script>"#;
        let result = rewrite_for_thread_id(html, "abc123");
        assert!(result.contains(r#"href="style.css?thread_id=abc123""#));
        assert!(result.contains(r#"src="app.js?thread_id=abc123""#));
    }

    #[test]
    fn thread_id_not_appended_to_absolute_or_special_urls() {
        let html = r##"<a href="https://example.com">x</a><a href="/api/v1/sdk.js">y</a><a href="#anchor">z</a>"##;
        let result = rewrite_for_thread_id(html, "abc123");
        assert!(
            !result.contains("?thread_id"),
            "absolute / anchor / root-path URLs must not be rewritten: {}",
            result
        );
    }

    #[test]
    fn rescope_app_html_prefixes_absolute_engine_refs() {
        let html = r#"<script src="/api/v1/sdk.js"></script><link href="/assets/x.css"><img src="/data/a.png">"#;
        let out = rescope_app_html(html, "/dev/");
        assert!(out.contains(r#"<script src="/dev/api/v1/sdk.js">"#));
        assert!(out.contains(r#"<link href="/dev/assets/x.css">"#));
        assert!(out.contains(r#"<img src="/dev/data/a.png">"#));
    }

    #[test]
    fn rescope_app_html_root_prefix_is_noop() {
        let html = r#"<script src="/api/v1/sdk.js"></script><link href="style.css">"#;
        assert_eq!(rescope_app_html(html, "/"), html);
    }

    #[test]
    fn rescope_app_html_leaves_relative_and_script_bodies_alone() {
        // App's own relative refs resolve against the iframe URL — untouched.
        // Inline-JS string literals containing src="/api/v1/…" must NOT change.
        let html = r#"<script src="/api/v1/sdk.js">var x='<img src="/api/v1/foo">';</script><link href="./style.css">"#;
        let out = rescope_app_html(html, "/work/");
        assert!(out.contains(r#"<script src="/work/api/v1/sdk.js">"#)); // opening tag rewritten
        assert!(out.contains(r#"var x='<img src="/api/v1/foo">';"#)); // body verbatim
        assert!(out.contains(r#"href="./style.css""#)); // relative untouched
    }

    #[test]
    fn thread_id_does_not_rewrite_script_body_template_literals() {
        // Template literals like `src="${var}"` inside <script> bodies must not be
        // rewritten — but the opening <script src=""> tag's own attrs are.
        let html =
            r#"<script src="lib.js"></script><script>var x = `<img src="foo.png">`;</script>"#;
        let result = rewrite_for_thread_id(html, "abc");
        assert!(
            result.contains(r#"src="lib.js?thread_id=abc""#),
            "opening tag attrs rewritten"
        );
        assert!(
            result.contains(r#"src="foo.png">"#),
            "script body untouched: {}",
            result
        );
        assert!(
            !result.contains(r#"foo.png?thread_id"#),
            "script body must not be rewritten"
        );
    }
}
