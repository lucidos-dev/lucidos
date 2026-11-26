static SCRIPT_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?si)(<script[\s][^>]*>|<script>)(.*?</script>)")
        .expect("app html script-block regex must compile")
});

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

const BRAND_FAVICON_LINKS: &str = concat!(
    r#"<link rel="icon" type="image/svg+xml" href="/favicon.svg">"#,
    r#"<link rel="icon" type="image/png" sizes="32x32" href="/favicon-32.png">"#
);

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

fn is_safe_device_id(device_id: &str) -> bool {
    const LOCAL_MAX: usize = 64;
    const RELAYED_REMOTE_MAX: usize = 128;
    let part_is_safe = |part: &str, max: usize, extra: Option<u8>| {
        !part.is_empty()
            && part.len() <= max
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || Some(b) == extra)
    };
    match device_id.split_once(':') {
        None => part_is_safe(device_id, LOCAL_MAX, None),
        Some((pairing_id, remote)) => {
            part_is_safe(pairing_id, LOCAL_MAX, None)
                && part_is_safe(remote, RELAYED_REMOTE_MAX, Some(b'_'))
        }
    }
}

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
    crate::core::base_path::insert_into_head(html, BRAND_FAVICON_LINKS)
}

pub(super) fn rewrite_for_thread_id(html: &str, thread_id: &str) -> String {
    let suffix = format!("?thread_id={}", thread_id);
    append_query_to_relative_paths(html, &suffix)
}

pub(super) fn rescope_app_html(html: &str, prefix: &str, capability: Option<&str>) -> String {
    if prefix == "/" {
        return html.to_string();
    }
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

fn carries_workspace_content(path: &str) -> bool {
    path.starts_with("/data/") || path.starts_with("/app/")
}

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
    let segment = lucidos_frame_capability::SEGMENT;
    let href = format!("{prefix}{segment}/{capability}/app/{app_id}/");
    let tag = format!(
        "<base href=\"{}\">",
        crate::core::base_path::escape_attr(&href)
    );
    crate::core::base_path::insert_into_head(html, &tag)
}

fn append_query_to_relative_paths(html: &str, suffix: &str) -> String {
    static ATTR_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"((?:src|href)\s*=\s*")([^"/][^"]*)"#)
            .expect("app thread-id attr regex must compile")
    });

    rewrite_outside_script_bodies(html, |fragment| {
        append_suffix_to_attrs(fragment, suffix, &ATTR_RE)
    })
}

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
        let html = r#"<script src="/api/v1/sdk.js">var x='<img src="/api/v1/foo">';</script><link href="./style.css">"#;
        let out = rescope_app_html(html, "/work/", None);
        assert!(out.contains(r#"<script src="/work/api/v1/sdk.js">"#));
        assert!(out.contains(r#"var x='<img src="/api/v1/foo">';"#));
        assert!(out.contains(r#"href="./style.css""#));
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
        let html = r#"<html><head><link rel="stylesheet" href="/api/v1/sdk-iframe.css"><link rel="apple-touch-icon" href="t.png"></head></html>"#;
        assert!(ensure_app_favicon(html).contains(r#"href="/favicon.svg""#));
    }

    #[test]
    fn the_stamped_favicon_is_not_pulled_into_a_wip_preview() {
        let out = rewrite_for_thread_id(&ensure_app_favicon("<html><head></head></html>"), "abc");
        assert!(out.contains(r#"href="/favicon.svg""#));
        assert!(!out.contains("favicon.svg?thread_id"));
    }

    #[test]
    fn thread_id_does_not_rewrite_script_body_template_literals() {
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
        let out = stamp_prefs_device(&rescope_app_html(OPTED_IN, "/dev/", None), "abc123");
        assert!(
            out.contains(r#"src="/dev/api/v1/sdk-prefs.js?device=abc123""#),
            "{out}"
        );
    }

    #[test]
    fn an_app_that_did_not_opt_in_is_byte_identical() {
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
        let out = stamp_prefs_device(OPTED_IN, r#"x" onload="alert(1)"#);
        assert_eq!(out, OPTED_IN, "refused whole, not partly escaped: {out}");
    }

    #[test]
    fn a_gateway_relayed_device_id_is_stamped() {
        let id = "0123456789abcdef:fedcba9876543210";
        let out = stamp_prefs_device(OPTED_IN, id);
        assert!(
            out.contains(&format!(r#"sdk-prefs.js?device={id}""#)),
            "{out}"
        );
    }

    #[test]
    fn a_relayed_remote_half_may_be_a_uuid_or_use_underscores() {
        assert!(is_safe_device_id(
            "0123456789abcdef:123e4567-e89b-12d3-a456-426614174000"
        ));
        assert!(is_safe_device_id("0123456789abcdef:phone_1"));
    }

    #[test]
    fn unsafe_ids_stay_refused() {
        for id in [
            "",
            ":",
            "0123456789abcdef:",
            ":fedcba9876543210",
            "a:b:c",
            "a:b\"c",
            "a:b<c",
            "a:b>c",
            "a:b/c",
            "a:b c",
            "a:b\\c",
            "a:b'c",
            "a:b&c",
            "a_b:c",
            "a b",
            "a\"b",
            "a_b",
        ] {
            assert!(!is_safe_device_id(id), "{id:?} must stay refused");
        }
        assert!(!is_safe_device_id(&"a".repeat(65)));
        assert!(!is_safe_device_id(&format!("a:{}", "b".repeat(129))));
    }

    #[test]
    fn a_script_body_mentioning_the_prefs_src_is_left_alone() {
        let html = r#"<script>var s = 'src="/api/v1/sdk-prefs.js"';</script>"#;
        assert_eq!(stamp_prefs_device(html, "abc123"), html);
    }

    const PASS: &str = "6a0b~site-publisher~00112233445566778899aabbccddeeff";

    #[test]
    fn the_base_carries_the_pass_to_every_relative_ref() {
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
    fn a_crafted_prefix_cannot_break_out_of_the_base_attribute() {
        let html = "<html><head></head><body></body></html>";
        let out = stamp_frame_capability(
            html,
            r#"/x"><script>alert(1)</script>/"#,
            PASS,
            "site-publisher",
        );
        assert!(!out.contains("<script>"), "{out}");
        assert!(out.contains("&quot;&gt;&lt;script&gt;"), "{out}");
    }

    #[test]
    fn an_app_that_declares_its_own_base_keeps_it() {
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
        assert!(out.contains(&format!(
            r#"src="/dev/~cap/{PASS}/data/artifacts/chart.png""#
        )));
        assert!(out.contains(&format!(
            r#"href="/dev/~cap/{PASS}/app/site-publisher/report.pdf""#
        )));
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
        let html = r#"<html><head><img src="/data/a.png"></head></html>"#;
        assert_eq!(rescope_app_html(html, "/", Some(PASS)), html);
        assert_eq!(rescope_app_html(html, "/", None), html);
    }

    #[test]
    fn the_wip_preview_suffix_rides_on_top_of_the_base() {
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
