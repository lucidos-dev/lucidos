pub(crate) use crate::desktop::LiveWindow;

fn is_workspace_slug(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('-')
        && !segment.ends_with('-')
        && !segment.contains("--")
        && segment
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn is_workspace_route(segment: &str) -> bool {
    match segment.split_once('@') {
        None => is_workspace_slug(segment),
        Some((slug, name)) => {
            is_workspace_slug(slug)
                && !name.is_empty()
                && !name.starts_with('-')
                && !name.ends_with('-')
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
    }
}

fn strip_http_scheme(url: &str) -> Option<&str> {
    ["http://", "https://"].into_iter().find_map(|prefix| {
        let (head, rest) = url.split_at_checked(prefix.len())?;
        head.eq_ignore_ascii_case(prefix).then_some(rest)
    })
}

pub(crate) fn window_origin(url: &str) -> Option<&str> {
    let rest = strip_http_scheme(url)?;
    let authority = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    (authority > 0).then(|| &url[..url.len() - rest.len() + authority])
}

pub(crate) fn window_workspace(url: &str) -> Option<&str> {
    let after_scheme = strip_http_scheme(url)?;
    let path = match after_scheme.find(['/', '?', '#']) {
        Some(i) if after_scheme.as_bytes()[i] == b'/' => &after_scheme[i + 1..],
        _ => return None,
    };
    let segment = &path[..path.find(['/', '?', '#']).unwrap_or(path.len())];
    is_workspace_route(segment).then_some(segment)
}

pub(crate) fn window_is_navigated(url: &str) -> bool {
    strip_http_scheme(url).is_some()
}

pub(crate) fn is_bundled_app_url(url: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("tauri"))
}

pub(crate) fn picker_url(origin: &str) -> String {
    format!("{origin}/~/?pick")
}

pub(crate) fn workspace_url(origin: &str, workspace: &str) -> String {
    format!("{origin}/{workspace}/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceLanding {
    Notifications,
    Thread(ThreadId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadId(String);

impl ThreadId {
    fn parse(id: &str) -> Option<Self> {
        let groups: Vec<&str> = id.split('-').collect();
        let canonical = groups.len() == 5
            && groups
                .iter()
                .zip([8, 4, 4, 4, 12])
                .all(|(group, len)| group.len() == len)
            && groups.iter().all(|g| {
                g.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        canonical.then(|| Self(id.to_string()))
    }
}

impl WorkspaceLanding {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        if let Some(id) = name.strip_prefix("thread:") {
            return ThreadId::parse(id).map(Self::Thread);
        }
        match name {
            "notifications" => Some(Self::Notifications),
            _ => None,
        }
    }

    fn fragment(&self) -> String {
        match self {
            Self::Notifications => "#notifications".to_string(),
            Self::Thread(ThreadId(id)) => format!("#thread={id}"),
        }
    }
}

fn landing_fragment(landing: Option<&WorkspaceLanding>) -> String {
    landing.map_or_else(String::new, WorkspaceLanding::fragment)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowContext<'a> {
    Unnavigated,
    Neutral,
    Workspace(&'a str),
}

pub(crate) fn window_context(url: &str) -> WindowContext<'_> {
    if let Some(workspace) = window_workspace(url) {
        return WindowContext::Workspace(workspace);
    }
    match strip_http_scheme(url) {
        Some(_) => WindowContext::Neutral,
        None => WindowContext::Unnavigated,
    }
}

pub(crate) fn target_origin<'a>(caller_url: Option<&'a str>, fallback: &'a str) -> &'a str {
    caller_url.and_then(window_origin).unwrap_or(fallback)
}

pub(crate) fn windows_in<'a>(
    windows: &'a [LiveWindow],
    want: WindowContext<'_>,
) -> Vec<&'a LiveWindow> {
    windows
        .iter()
        .filter(|window| window_context(&window.url) == want)
        .collect()
}

pub(crate) fn preferred_label<'a>(windows: &[&'a LiveWindow]) -> Option<&'a str> {
    windows
        .iter()
        .min_by_key(|window| {
            (
                !(window.visible || window.minimized),
                std::cmp::Reverse(window.last_focused),
                window.label != crate::app_window::MAIN_WINDOW_LABEL,
                window.label.as_str(),
            )
        })
        .map(|window| window.label.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceTarget {
    Focus(String),
    Navigate { label: String, url: String },
    NewWindow { url: String },
}

pub(crate) fn choose_workspace_target(
    windows: &[LiveWindow],
    caller_label: &str,
    workspace: &str,
    origin: &str,
    landing: Option<WorkspaceLanding>,
) -> Option<WorkspaceTarget> {
    if !is_workspace_route(workspace) {
        return None;
    }
    let url = format!(
        "{}{}",
        workspace_url(origin, workspace),
        landing_fragment(landing.as_ref())
    );

    let on_workspace = windows_in(windows, WindowContext::Workspace(workspace));
    if let Some(label) = preferred_label(&on_workspace) {
        return Some(match landing {
            Some(_) => WorkspaceTarget::Navigate {
                label: label.to_string(),
                url,
            },
            None => WorkspaceTarget::Focus(label.to_string()),
        });
    }

    let caller_is_neutral = windows.iter().any(|window| {
        window.label == caller_label && window_context(&window.url) == WindowContext::Neutral
    });
    if caller_is_neutral {
        return Some(WorkspaceTarget::Navigate {
            label: caller_label.to_string(),
            url,
        });
    }

    Some(WorkspaceTarget::NewWindow { url })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn window_context_reads_the_workspace_out_of_a_window_url() {
        assert_eq!(
            window_context("http://localhost:3210/myws/"),
            WindowContext::Workspace("myws")
        );
        assert_eq!(
            window_context("https://host:8443/my-ws-2/#thread=abc"),
            WindowContext::Workspace("my-ws-2")
        );
        assert_eq!(
            window_context("http://localhost:3210/~/"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("http://localhost:3210/~/?pick"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("http://localhost:3210/"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("http://localhost:3210"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("http://localhost:3210?x=1"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("http://localhost:3210/Not_A_Slug/"),
            WindowContext::Neutral
        );
        assert_eq!(
            window_context("tauri://localhost"),
            WindowContext::Unnavigated
        );
        assert_eq!(window_context(""), WindowContext::Unnavigated);
    }

    #[test]
    fn window_origin_keeps_the_authority_and_refuses_a_non_http_url() {
        assert_eq!(
            window_origin("http://localhost:3210"),
            Some("http://localhost:3210")
        );
        assert_eq!(
            window_origin("https://host.example:8443/~/?pick"),
            Some("https://host.example:8443")
        );
        assert_eq!(
            window_origin("http://[::1]:3210/myws/"),
            Some("http://[::1]:3210")
        );
        assert_eq!(window_origin("http://"), None);
        assert_eq!(window_origin("tauri://localhost"), None);
        assert_eq!(window_origin(""), None);
    }

    #[test]
    fn window_is_navigated_splits_the_boot_splash_from_the_picker() {
        assert!(!window_is_navigated("tauri://localhost"));
        assert!(!window_is_navigated(""));
        assert!(window_is_navigated("http://localhost:3210/~/"));
        assert!(window_is_navigated("http://localhost:3210/myws/"));
    }

    #[test]
    fn is_bundled_app_url_names_only_the_tauri_scheme() {
        assert!(is_bundled_app_url("tauri://localhost"));
        assert!(is_bundled_app_url("TAURI://localhost/index.html"));
        for other in [
            "http://localhost:3210/myws/",
            "about:blank",
            "blob:http://localhost:3210/x",
            "",
        ] {
            assert!(!is_bundled_app_url(other), "{other}");
        }
    }

    #[test]
    fn a_remote_route_is_a_slug_and_a_gateway_name() {
        assert!(is_workspace_route("myws@cloud"));
        assert!(is_workspace_route("my-ws@box-2"));
        for bad in [
            "myws@",
            "@cloud",
            "@",
            "MyWs@cloud",
            "myws@Cloud",
            "myws@cl@oud",
            "myws@-x",
            "myws@a/b",
        ] {
            assert!(!is_workspace_route(bad), "{bad}");
        }
        assert_eq!(
            window_workspace("http://localhost:3210/myws@cloud/#thread=x"),
            Some("myws@cloud")
        );
    }

    #[test]
    fn is_workspace_route_matches_what_the_gateway_would_serve() {
        assert!(is_workspace_route("myws"));
        assert!(is_workspace_route("my-ws-2"));
        assert!(!is_workspace_route("~"));
        assert!(!is_workspace_route(""));
        assert!(!is_workspace_route("-ws"));
        assert!(!is_workspace_route("ws-"));
        assert!(!is_workspace_route("my--ws"));
        assert!(!is_workspace_route("MyWs"));
        assert!(!is_workspace_route("my ws"));
        assert!(!is_workspace_route("my/ws"));
        assert!(!is_workspace_route(".."));
    }

    #[test]
    fn workspace_url_composes_the_origin_and_the_slug() {
        assert_eq!(
            workspace_url("http://localhost:3210", "myws"),
            "http://localhost:3210/myws/"
        );
    }

    pub(crate) fn windows(pairs: &[(&str, &str)]) -> Vec<LiveWindow> {
        pairs
            .iter()
            .map(|(label, url)| LiveWindow {
                label: (*label).to_string(),
                url: (*url).to_string(),
                visible: true,
                dismissed: false,
                minimized: false,
                last_focused: None,
            })
            .collect()
    }

    pub(crate) fn hidden(mut windows: Vec<LiveWindow>, label: &str) -> Vec<LiveWindow> {
        for window in windows.iter_mut().filter(|w| w.label == label) {
            window.visible = false;
        }
        windows
    }

    #[test]
    fn a_visible_window_beats_a_hidden_main_on_the_same_workspace() {
        let open = hidden(
            windows(&[
                ("main", "http://localhost:3210/personal/"),
                ("window-1", "http://localhost:3210/dev/"),
                ("window-2", "http://localhost:3210/personal/"),
            ]),
            "main",
        );
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-2".to_string()))
        );
        assert_eq!(
            choose_workspace_target(
                &open,
                "window-1",
                "personal",
                ORIGIN,
                Some(WorkspaceLanding::Notifications)
            ),
            Some(WorkspaceTarget::Navigate {
                label: "window-2".to_string(),
                url: "http://localhost:3210/personal/#notifications".to_string(),
            })
        );
        let open = hidden(open, "window-2");
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("main".to_string()))
        );
    }

    #[test]
    fn a_minimized_window_beats_a_hidden_main_on_the_same_workspace() {
        let mut open = hidden(
            windows(&[
                ("main", "http://localhost:3210/personal/"),
                ("window-1", "http://localhost:3210/dev/"),
                ("window-2", "http://localhost:3210/personal/"),
            ]),
            "main",
        );
        let parked = open.iter_mut().find(|w| w.label == "window-2").unwrap();
        parked.visible = false;
        parked.minimized = true;
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-2".to_string()))
        );
    }

    fn focused(mut windows: Vec<LiveWindow>, label: &str, seq: u64) -> Vec<LiveWindow> {
        for window in windows.iter_mut().filter(|w| w.label == label) {
            window.last_focused = Some(seq);
        }
        windows
    }

    #[test]
    fn the_window_focused_last_on_a_workspace_wins() {
        let open = windows(&[
            ("main", "http://localhost:3210/personal/"),
            ("window-1", "http://localhost:3210/dev/"),
            ("window-2", "http://localhost:3210/personal/"),
        ]);
        let open = focused(focused(open, "main", 1), "window-2", 2);
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-2".to_string()))
        );
        let open = focused(open, "main", 3);
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("main".to_string()))
        );
    }

    #[test]
    fn a_closed_window_never_beats_a_shown_one_however_recent() {
        let open = windows(&[
            ("main", "http://localhost:3210/personal/"),
            ("window-1", "http://localhost:3210/dev/"),
            ("window-2", "http://localhost:3210/personal/"),
        ]);
        let open = hidden(focused(focused(open, "window-2", 1), "main", 2), "main");
        assert_eq!(
            choose_workspace_target(&open, "window-1", "personal", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-2".to_string()))
        );
    }

    const ORIGIN: &str = "http://localhost:3210";
    const FALLBACK: &str = "http://localhost:3210";

    #[test]
    fn a_row_click_focuses_the_window_already_on_that_workspace() {
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/work/#thread=t1"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
        assert_eq!(
            choose_workspace_target(&open, "window-1", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
    }

    #[test]
    fn main_wins_when_several_windows_are_on_the_clicked_workspace() {
        let open = windows(&[
            ("window-2", "http://localhost:3210/work/"),
            ("main", "http://localhost:3210/work/"),
            ("window-1", "http://localhost:3210/work/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "window-2", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("main".to_string()))
        );
        let open = windows(&[
            ("window-2", "http://localhost:3210/work/"),
            ("window-1", "http://localhost:3210/work/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "window-2", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
    }

    #[test]
    fn a_click_in_the_picker_navigates_the_picker_window_itself() {
        let open = windows(&[("main", "http://localhost:3210/~/")]);
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::Navigate {
                label: "main".to_string(),
                url: "http://localhost:3210/work/".to_string(),
            })
        );
    }

    #[test]
    fn another_windows_picker_is_left_alone() {
        let open = windows(&[
            ("main", "http://localhost:3210/~/"),
            ("window-1", "http://localhost:3210/dev/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "window-1", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
    }

    #[test]
    fn a_remote_workspace_opens_beside_the_local_window_it_was_clicked_from() {
        let open = windows(&[("main", "http://localhost:3210/dev/")]);
        assert_eq!(
            choose_workspace_target(&open, "main", "dev@cloud", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/dev@cloud/".to_string(),
            })
        );
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/dev@cloud/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "main", "dev@cloud", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
    }

    #[test]
    fn an_unnavigated_window_is_never_the_target() {
        let open = windows(&[
            ("main", "tauri://localhost"),
            ("window-1", "http://localhost:3210/dev/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "window-1", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
    }

    #[test]
    fn a_click_from_another_workspace_opens_a_window_rather_than_taking_one() {
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/third/"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
        assert_eq!(
            choose_workspace_target(&[], "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
    }

    #[test]
    fn a_new_workspace_window_takes_the_callers_own_origin() {
        let at = |caller: Option<&str>| {
            choose_workspace_target(&[], "main", "work", target_origin(caller, FALLBACK), None)
        };
        assert_eq!(
            at(Some("http://localhost:3210/dev/?pick")),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
        assert_eq!(
            at(Some("https://box.tailnet.ts.net/dev/")),
            Some(WorkspaceTarget::NewWindow {
                url: "https://box.tailnet.ts.net/work/".to_string(),
            })
        );
        assert_eq!(
            at(Some("http://localhost:5173/~/")),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:5173/work/".to_string(),
            })
        );
        for caller in [None, Some("tauri://localhost"), Some("")] {
            assert_eq!(
                at(caller),
                Some(WorkspaceTarget::NewWindow {
                    url: "http://localhost:3210/work/".to_string(),
                }),
                "caller {caller:?}"
            );
        }
    }

    #[test]
    fn a_workspace_that_is_not_a_slug_targets_nothing() {
        let open = windows(&[("main", "http://localhost:3210/dev/")]);
        for bad in [
            "",
            "..",
            "../../etc",
            "~",
            "work/../dev",
            "Work",
            "work space",
            "http://evil.example.com",
            "-work",
            "work-",
        ] {
            assert_eq!(
                choose_workspace_target(&open, "main", bad, ORIGIN, None),
                None,
                "{bad:?} was accepted"
            );
        }
    }

    #[test]
    fn a_landing_that_is_not_a_name_this_build_serves_is_refused() {
        assert_eq!(
            WorkspaceLanding::parse("notifications"),
            Some(WorkspaceLanding::Notifications)
        );
        for bad in [
            "",
            "#notifications",
            "notification",
            "Notifications",
            " notifications",
            "notifications#",
            "../etc",
            "threads",
        ] {
            assert_eq!(WorkspaceLanding::parse(bad), None, "{bad:?} was accepted");
        }
    }

    #[test]
    fn the_notifications_landing_is_the_bare_hash_the_page_router_reads() {
        assert_eq!(WorkspaceLanding::Notifications.fragment(), "#notifications");
        assert_eq!(landing_fragment(None), "");
    }

    const THREAD: &str = "3448b7a4-52d9-4243-8b17-58a7364484ed";

    #[test]
    fn the_thread_landing_is_the_thread_hash_the_page_router_reads() {
        let landing = WorkspaceLanding::parse(&format!("thread:{THREAD}")).unwrap();
        assert_eq!(landing.fragment(), format!("#thread={THREAD}"));
    }

    #[test]
    fn a_thread_landing_accepts_only_a_canonical_uuid() {
        for bad in [
            "thread:",
            "thread",
            "thread:3448B7A4-52D9-4243-8B17-58A7364484ED",
            "thread:{3448b7a4-52d9-4243-8b17-58a7364484ed}",
            "thread:3448b7a452d942438b1758a7364484ed",
            "thread:urn:uuid:3448b7a4-52d9-4243-8b17-58a7364484ed",
            "thread:3448b7a4-52d9-4243-8b17-58a7364484e",
            "thread:3448b7a45-2d9-4243-8b17-58a7364484ed",
            "thread:3448b7a4-52d9-4243-8b17-58a7364484eg",
            "thread:3448b7a4-52d9-4243-8b17-58a7364484ed#notifications",
            "thread:3448b7a4-52d9-4243-8b17-58a7364484ed/../x",
            " thread:3448b7a4-52d9-4243-8b17-58a7364484ed",
            "Thread:3448b7a4-52d9-4243-8b17-58a7364484ed",
            "thread=3448b7a4-52d9-4243-8b17-58a7364484ed",
        ] {
            assert_eq!(WorkspaceLanding::parse(bad), None, "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_landing_rides_every_target_the_chooser_can_pick() {
        let landing = || Some(WorkspaceLanding::Notifications);
        assert_eq!(
            choose_workspace_target(&[], "main", "work", ORIGIN, landing()),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/#notifications".to_string(),
            })
        );
        let picker = windows(&[("main", "http://localhost:3210/~/")]);
        assert_eq!(
            choose_workspace_target(&picker, "main", "work", ORIGIN, landing()),
            Some(WorkspaceTarget::Navigate {
                label: "main".to_string(),
                url: "http://localhost:3210/work/#notifications".to_string(),
            })
        );
    }

    #[test]
    fn a_landing_navigates_the_window_already_on_the_workspace_rather_than_opening_one() {
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/work/#thread=t1"),
        ]);
        assert_eq!(
            choose_workspace_target(
                &open,
                "main",
                "work",
                ORIGIN,
                Some(WorkspaceLanding::Notifications)
            ),
            Some(WorkspaceTarget::Navigate {
                label: "window-1".to_string(),
                url: "http://localhost:3210/work/#notifications".to_string(),
            })
        );
        assert_eq!(
            choose_workspace_target(
                &open,
                "main",
                "work",
                ORIGIN,
                WorkspaceLanding::parse(&format!("thread:{THREAD}"))
            ),
            Some(WorkspaceTarget::Navigate {
                label: "window-1".to_string(),
                url: format!("http://localhost:3210/work/#thread={THREAD}"),
            })
        );
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
    }

    #[test]
    fn a_landing_does_not_rescue_a_slug_the_gateway_would_not_serve() {
        assert_eq!(
            choose_workspace_target(
                &[],
                "main",
                "../etc",
                ORIGIN,
                Some(WorkspaceLanding::Notifications)
            ),
            None
        );
    }
}
