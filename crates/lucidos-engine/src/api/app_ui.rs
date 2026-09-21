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

/// The brand tab icon stamped into an app document that declares none. Written
/// as root-absolute refs, so [`rescope_app_html`] carries them to this
/// workspace behind the gateway, and the engine's own `dist/` serves them on a
/// direct hit.
const BRAND_FAVICON_LINKS: &str = concat!(
    r#"<link rel="icon" type="image/svg+xml" href="/favicon.svg">"#,
    r#"<link rel="icon" type="image/png" sizes="32x32" href="/favicon-32.png">"#
);

/// Tell `/api/v1/sdk-prefs.js` which device is asking, by stamping `?device=`
/// onto the app's own reference to it.
///
/// That script resolves this device's theme, font and UI scale, and it is
/// parser-blocking, so nothing async can feed it. It used to read them out of
/// `localStorage`, which an app frame could see only because its sandbox said
/// `allow-same-origin`. Isolating the frame takes that away.
///
/// **Nothing is added to the document.** No tag is injected, and no markup the
/// app wrote is touched. The one edit is to how an engine route addresses
/// itself. [`rescope_app_html`] already edits this same `src`, for the
/// workspace prefix (ADR 0014 §4).
///
/// So styling stays opt-in, and the opt-in stays the app's own tag rather than
/// anything inferred about it. An app that never asks for the script comes back
/// byte-identical.
pub(super) fn stamp_prefs_device(html: &str, device_id: &str) -> String {
    static PREFS_SRC_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(src\s*=\s*")([^"?]*/api/v1/sdk-prefs\.js)(")"#)
            .expect("app prefs-src regex must compile")
    });
    if !is_safe_device_id(device_id) {
        return html.to_string();
    }
    rewrite_outside_script_bodies(html, |fragment| {
        PREFS_SRC_RE
            .replace_all(fragment, |caps: &regex::Captures| {
                format!("{}{}?device={}{}", &caps[1], &caps[2], device_id, &caps[3])
            })
            .into_owned()
    })
}

/// A device id safe to splice into an attribute value without escaping.
///
/// The id is minted by the client and arrives as a query param, so it is an
/// untrusted string reaching HTML. Rather than escape it, refuse anything that
/// is not the shape a device id has. The two live forms are a uuid and a hex
/// token. A refusal costs the seed, which the app then corrects for itself.
fn is_safe_device_id(device_id: &str) -> bool {
    !device_id.is_empty()
        && device_id.len() <= 64
        && device_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Give an app document the Lucidos tab icon unless it names its own.
///
/// An app opened in its own browser tab is a top-level document, and it lives
/// at `/<slug>/app/<id>/` rather than at the origin root. A browser with no
/// `<link rel="icon">` to follow probes the root for `/favicon.ico`. That root
/// is the gateway, not any workspace, so the tab falls back to the blank page
/// glyph. Stamping the links here covers every access mode at once. An iframe
/// ignores them, so the inline app surface is unaffected.
pub(super) fn ensure_app_favicon(html: &str) -> String {
    static LINK_REL_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)<link\b[^>]*?\brel\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
            .expect("app favicon rel regex must compile")
    });
    let declares_icon = LINK_REL_RE.captures_iter(html).any(|caps| {
        (1..=3)
            .filter_map(|i| caps.get(i))
            .flat_map(|m| m.as_str().split_whitespace())
            .any(|token| token.eq_ignore_ascii_case("icon"))
    });
    if declares_icon {
        return html.to_string();
    }
    super::base_path::insert_into_head(html, BRAND_FAVICON_LINKS)
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
/// refs (`./style.css`) already resolve against the base stamped by
/// [`stamp_frame_capability`], and `<script>` BODIES are skipped so inline-JS
/// string literals aren't corrupted. Idempotent in practice (already-prefixed
/// paths don't re-match).
///
/// `capability` threads the frame's URL pass onto the two prefixes that carry
/// workspace content, `/data/` and `/app/` (ADR 0238). A root-absolute ref to
/// one of those is the app addressing its own files, and behind a gateway those
/// need the pass. The engine's own `/api/v1` assets are exempt by name and get
/// none. A pass reaching `/api/v1` would put the whole API behind a token the
/// frame hands to every document it embeds.
pub(super) fn rescope_app_html(html: &str, prefix: &str, capability: Option<&str>) -> String {
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

    let carrier = capability
        .map(|token| format!("/{}/{token}", lucidos_frame_capability::SEGMENT))
        .unwrap_or_default();

    rewrite_outside_script_bodies(html, |fragment| {
        ATTR_RE
            .replace_all(fragment, |caps: &regex::Captures| {
                let path = &caps[2];
                let pass = if carries_workspace_content(path) {
                    carrier.as_str()
                } else {
                    ""
                };
                format!("{}{}{}{}", &caps[1], slug, pass, path)
            })
            .into_owned()
    })
}

/// Does this root-absolute ref address the workspace, rather than the engine?
///
/// The two trees a frame capability reaches, and the only two that need one.
fn carries_workspace_content(path: &str) -> bool {
    path.starts_with("/data/") || path.starts_with("/app/")
}

/// Give a framed app document a `<base href>` carrying its capability.
///
/// This is the whole of how an app's OWN relative refs work behind a gateway.
/// `./style.css`, `<img src="logo.png">` and a nested iframe all resolve against
/// the base, so each picks the pass up with no attribute rewritten.
///
/// It is also the one handle a renewal can move. The SDK swaps a fresh token
/// into this element when the host pushes one, and nothing reloads.
/// `history.replaceState` would have been the alternative, and it throws at an
/// opaque origin on WebKit.
///
/// **A renewal reaches what resolves against the DOCUMENT, and nothing else.**
/// A dynamic `import()` inside an ES module resolves against that module's own
/// url, a stylesheet's `url()` against the stylesheet's. Both keep the pass
/// they loaded with. ADR 0238 § Consequences carries the class.
///
/// An app that declares its own `<base href>` keeps it and gets none. The first
/// base in a document wins, so inserting ours would silently override a choice
/// the author made. Such an app loads as it does today.
pub(super) fn stamp_frame_capability(
    html: &str,
    prefix: &str,
    capability: &str,
    app_id: &str,
) -> String {
    static BASE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)<base\b[^>]*?\bhref\s*=\s*"#).expect("app base-href regex")
    });
    if BASE_RE.is_match(html) {
        return html.to_string();
    }
    // `prefix` reaches us from `X-Forwarded-Prefix`, which is the gateway's on a
    // proxied request and forgeable on a direct hit to the engine's own port.
    // `frame_capability::mint` refuses a prefix that is not slug-shaped, so
    // there is no pass to stamp when one is crafted. Escaped anyway, because
    // that guarantee lives in another function and `inject_base_href` escapes
    // the same value for the same reason.
    let segment = lucidos_frame_capability::SEGMENT;
    let href = format!("{prefix}{segment}/{capability}/app/{app_id}/");
    let tag = format!("<base href=\"{}\">", super::base_path::escape_attr(&href));
    super::base_path::insert_into_head(html, &tag)
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
        let out = rescope_app_html(html, "/dev/", None);
        assert!(out.contains(r#"<script src="/dev/api/v1/sdk.js">"#));
        assert!(out.contains(r#"<link href="/dev/assets/x.css">"#));
        assert!(out.contains(r#"<img src="/dev/data/a.png">"#));
    }

    #[test]
    fn rescope_app_html_root_prefix_is_noop() {
        let html = r#"<script src="/api/v1/sdk.js"></script><link href="style.css">"#;
        assert_eq!(rescope_app_html(html, "/", None), html);
    }

    #[test]
    fn rescope_app_html_leaves_relative_and_script_bodies_alone() {
        // App's own relative refs resolve against the iframe URL — untouched.
        // Inline-JS string literals containing src="/api/v1/…" must NOT change.
        let html = r#"<script src="/api/v1/sdk.js">var x='<img src="/api/v1/foo">';</script><link href="./style.css">"#;
        let out = rescope_app_html(html, "/work/", None);
        assert!(out.contains(r#"<script src="/work/api/v1/sdk.js">"#)); // opening tag rewritten
        assert!(out.contains(r#"var x='<img src="/api/v1/foo">';"#)); // body verbatim
        assert!(out.contains(r#"href="./style.css""#)); // relative untouched
    }

    #[test]
    fn an_app_that_names_no_icon_gets_the_brand_favicon_in_its_head() {
        let html = r#"<!DOCTYPE html><html><head><title>Habit Tracker</title></head><body><header>h</header></body></html>"#;
        let out = ensure_app_favicon(html);
        assert!(out.contains(r#"<head><link rel="icon" type="image/svg+xml" href="/favicon.svg">"#));
        assert!(out.contains(r#"href="/favicon-32.png""#));
        assert!(out.contains("<title>Habit Tracker</title>"));
    }

    #[test]
    fn the_stamped_favicon_follows_the_workspace_prefix_behind_the_gateway() {
        let out = rescope_app_html(
            &ensure_app_favicon("<html><head></head></html>"),
            "/dev/",
            None,
        );
        assert!(out.contains(r#"href="/dev/favicon.svg""#));
        assert!(out.contains(r#"href="/dev/favicon-32.png""#));
    }

    #[test]
    fn an_app_that_names_its_own_icon_keeps_it() {
        for rel in [r#""icon""#, r#""shortcut icon""#, "icon", r#"'ICON'"#] {
            let html = format!(r#"<html><head><link rel={rel} href="logo.png"></head></html>"#);
            assert_eq!(ensure_app_favicon(&html), html, "rel={rel}");
        }
    }

    #[test]
    fn a_link_that_is_not_an_icon_does_not_count_as_one() {
        // `apple-touch-icon` is a home-screen icon, never a tab icon, so it
        // leaves the tab needing ours.
        let html = r#"<html><head><link rel="stylesheet" href="/api/v1/sdk-iframe.css"><link rel="apple-touch-icon" href="t.png"></head></html>"#;
        assert!(ensure_app_favicon(html).contains(r#"href="/favicon.svg""#));
    }

    #[test]
    fn the_stamped_favicon_is_not_pulled_into_a_wip_preview() {
        // Root-absolute, so the preview's relative-ref suffix skips it. The
        // brand icon is engine-served, not part of the app's worktree.
        let out = rewrite_for_thread_id(&ensure_app_favicon("<html><head></head></html>"), "abc");
        assert!(out.contains(r#"href="/favicon.svg""#));
        assert!(!out.contains("favicon.svg?thread_id"));
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

    const OPTED_IN: &str =
        r#"<html><head><script src="/api/v1/sdk-prefs.js"></script></head><body></body></html>"#;

    #[test]
    fn prefs_src_carries_the_device() {
        let out = stamp_prefs_device(OPTED_IN, "cfdb4cbfe6044f8b");
        assert!(
            out.contains(r#"<script src="/api/v1/sdk-prefs.js?device=cfdb4cbfe6044f8b">"#),
            "{out}"
        );
    }

    #[test]
    fn the_rescoped_prefs_src_still_matches() {
        // Behind the gateway the src is already `/dev/api/v1/…` by the time this
        // runs, so the pattern cannot be anchored at the start of the path.
        let out = stamp_prefs_device(&rescope_app_html(OPTED_IN, "/dev/", None), "abc123");
        assert!(
            out.contains(r#"src="/dev/api/v1/sdk-prefs.js?device=abc123""#),
            "{out}"
        );
    }

    #[test]
    fn an_app_that_did_not_opt_in_is_byte_identical() {
        // Styling is opt-in, and the opt-in is the app's own tag. An app that
        // names no prefs script gets nothing: no seed, no injected element, no
        // edit of any kind.
        let html =
            r#"<html><head><title>Plain</title><script src="app.js"></script></head></html>"#;
        assert_eq!(stamp_prefs_device(html, "abc123"), html);
    }

    #[test]
    fn nothing_but_the_prefs_src_is_rewritten() {
        let html = r#"<script src="/api/v1/sdk.js"></script><script src="/api/v1/sdk-prefs.js"></script><link href="/api/v1/sdk-iframe.css">"#;
        let out = stamp_prefs_device(html, "abc123");
        assert!(
            out.contains(r#"src="/api/v1/sdk.js""#),
            "sdk.js untouched: {out}"
        );
        assert!(
            out.contains(r#"href="/api/v1/sdk-iframe.css""#),
            "the stylesheet is not a src and stays put: {out}"
        );
        assert_eq!(
            out.matches("?device=").count(),
            1,
            "exactly one stamp: {out}"
        );
    }

    #[test]
    fn a_device_id_that_is_not_one_is_refused_rather_than_escaped() {
        // The id reaches HTML from a query param, so it is untrusted input. A
        // quote in it would close the attribute and open a new one.
        let out = stamp_prefs_device(OPTED_IN, r#"x" onload="alert(1)"#);
        assert_eq!(out, OPTED_IN, "refused whole, not partly escaped: {out}");
    }

    #[test]
    fn a_script_body_mentioning_the_prefs_src_is_left_alone() {
        let html = r#"<script>var s = 'src="/api/v1/sdk-prefs.js"';</script>"#;
        assert_eq!(stamp_prefs_device(html, "abc123"), html);
    }

    // ── The frame capability (ADR 0238) ─────────────────────────────────────

    const PASS: &str = "6a0b~site-publisher~00112233445566778899aabbccddeeff";

    #[test]
    fn the_base_carries_the_pass_to_every_relative_ref() {
        // The app writes nothing special. `./style.css` resolves against this
        // base, and so does a nested iframe and a runtime import.
        let html =
            r#"<!DOCTYPE html><html><head><link href="./style.css"></head><body></body></html>"#;
        let out = stamp_frame_capability(html, "/dev/", PASS, "site-publisher");
        assert!(
            out.contains(&format!(
                r#"<head><base href="/dev/~cap/{PASS}/app/site-publisher/">"#
            )),
            "{out}"
        );
        assert!(
            out.contains(r#"<link href="./style.css">"#),
            "the app's own markup is untouched: {out}"
        );
    }

    #[test]
    fn an_app_that_declares_its_own_base_keeps_it() {
        // The first base in a document wins, so ours would silently override a
        // choice the author made. Such an app loads as it does today.
        for declared in [
            r#"<base href="./">"#,
            r#"<base target="_top" href="/dev/app/x/">"#,
            r#"<BASE HREF='./sub/'>"#,
        ] {
            let html = format!("<html><head>{declared}</head></html>");
            assert_eq!(
                stamp_frame_capability(&html, "/dev/", PASS, "site-publisher"),
                html,
                "{declared}"
            );
        }
    }

    #[test]
    fn the_pass_reaches_workspace_refs_and_no_engine_route() {
        let html = concat!(
            r#"<script src="/api/v1/sdk.js"></script>"#,
            r#"<link href="/api/v1/sdk-iframe.css">"#,
            r#"<img src="/data/artifacts/chart.png">"#,
            r#"<a href="/app/site-publisher/report.pdf" download>r</a>"#,
            r#"<link href="/assets/x.css"><link href="/favicon.svg">"#,
        );
        let out = rescope_app_html(html, "/dev/", Some(PASS));
        // Workspace content, which needs the pass behind a gateway.
        assert!(out.contains(&format!(
            r#"src="/dev/~cap/{PASS}/data/artifacts/chart.png""#
        )));
        assert!(out.contains(&format!(
            r#"href="/dev/~cap/{PASS}/app/site-publisher/report.pdf""#
        )));
        // Engine routes and bundle assets, which must never carry one.
        assert!(out.contains(r#"src="/dev/api/v1/sdk.js""#), "{out}");
        assert!(
            out.contains(r#"href="/dev/api/v1/sdk-iframe.css""#),
            "{out}"
        );
        assert!(out.contains(r#"href="/dev/assets/x.css""#), "{out}");
        assert!(out.contains(r#"href="/dev/favicon.svg""#), "{out}");
        assert_eq!(
            out.matches("~cap").count(),
            2,
            "exactly the two workspace refs: {out}"
        );
    }

    #[test]
    fn a_direct_hit_gets_no_pass_and_no_base() {
        // No gateway means no device gate, so there is nothing to prove and
        // nothing is added. Every other rewrite here has the same shape.
        let html = r#"<html><head><img src="/data/a.png"></head></html>"#;
        assert_eq!(rescope_app_html(html, "/", Some(PASS)), html);
        assert_eq!(rescope_app_html(html, "/", None), html);
    }

    #[test]
    fn the_wip_preview_suffix_rides_on_top_of_the_base() {
        // A preview rewrites relative refs to carry `?thread_id=`, and those
        // then resolve against the capability base. Both survive.
        let html = r#"<html><head><link href="style.css"></head></html>"#;
        let out = stamp_frame_capability(
            &rescope_app_html(&rewrite_for_thread_id(html, "abc"), "/dev/", Some(PASS)),
            "/dev/",
            PASS,
            "site-publisher",
        );
        assert!(out.contains(r#"href="style.css?thread_id=abc""#), "{out}");
        assert!(
            out.contains(&format!(r#"<base href="/dev/~cap/{PASS}/app/"#)),
            "{out}"
        );
    }
}
