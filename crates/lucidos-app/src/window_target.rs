//! What a client window's URL says about where it is pointed.
//!
//! One packaged process fronts the gateway, and each window can sit on its own
//! workspace (ADR 0014). So "which window is on what" is a real question, with
//! several callers. A native banner tap routes by it
//! (`notifications::choose_tap_target`). The window session records the user's
//! arrangement by it (ADR 0123), and `desktop` composes launch URLs from it.
//!
//! Everything here is pure and platform-independent. That is what makes the
//! decisions built on it unit-testable off macOS, where the tap delegate driving
//! one of those callers cannot run at all.

/// A workspace slug the gateway would serve. Mirrors `SLUG_RE` in
/// `utils/basePath.ts` (lowercase alphanumerics joined by single hyphens, no
/// leading/trailing hyphen), which is itself `registry::slugify`'s output shape.
/// The gateway's reserved sigil (`/~/…`, ADR 0014) fails it on the character
/// class, which is why nothing tests for `~` separately.
pub(crate) fn is_workspace_slug(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('-')
        && !segment.ends_with('-')
        && !segment.contains("--")
        && segment
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Everything after `http://` / `https://`, or `None` for any other scheme.
/// A non-http URL is the bundled app URL, i.e. a window that has not been
/// navigated to the gateway.
fn strip_http_scheme(url: &str) -> Option<&str> {
    ["http://", "https://"].into_iter().find_map(|prefix| {
        let (head, rest) = url.split_at_checked(prefix.len())?;
        head.eq_ignore_ascii_case(prefix).then_some(rest)
    })
}

/// The `scheme://host[:port]` a window is served off, or `None` when it is not
/// on an http(s) URL at all (an unnavigated window). Backs the origin every
/// target URL is built on: the client is normally on the stable loopback
/// gateway, but a window reached some other way should target itself.
///
/// **Deliberately NOT `tauri::Url::origin()`.** That returns the *opaque* origin
/// for any non-special scheme, which serializes to the literal `"null"`. The one
/// URL a window is most likely to be on during startup is `tauri://localhost`.
/// Building `null/<slug>/` from it produces a URL nothing can parse. So
/// `desktop::launch` would fail to navigate, and leave the client on the boot
/// splash for good. Restricting to http(s) by construction makes that
/// unrepresentable.
pub(crate) fn window_origin(url: &str) -> Option<&str> {
    let rest = strip_http_scheme(url)?;
    let authority = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    (authority > 0).then(|| &url[..url.len() - rest.len() + authority])
}

/// The workspace a window is serving, or `None` when it is serving none.
///
/// `None` covers both non-workspace shapes at once, because most callers treat
/// them alike: an unnavigated window still on the bundled app URL, and one on
/// the gateway root or the `~` sigil. [`window_context`] is the same rule with
/// the two told apart, for the callers that need it.
pub(crate) fn window_workspace(url: &str) -> Option<&str> {
    let after_scheme = strip_http_scheme(url)?;
    // The authority runs to the first '/', '?' or '#'. Anything but a '/' means
    // there is no path at all (`http://host`, `http://host?x`), i.e. the root.
    let path = match after_scheme.find(['/', '?', '#']) {
        Some(i) if after_scheme.as_bytes()[i] == b'/' => &after_scheme[i + 1..],
        _ => return None,
    };
    let segment = &path[..path.find(['/', '?', '#']).unwrap_or(path.len())];
    is_workspace_slug(segment).then_some(segment)
}

/// Has this window reached the gateway at all?
///
/// False for the bundled `tauri://localhost` every window starts on, and true
/// from the moment it is navigated, workspace or picker alike. `window_session`
/// needs the distinction: a window still on the splash says nothing about the
/// user's arrangement, while one sitting on the picker says they left it there.
pub(crate) fn window_is_navigated(url: &str) -> bool {
    strip_http_scheme(url).is_some()
}

/// The gateway URL serving `workspace`, e.g. `http://localhost:3210/myws/`.
/// `origin` carries no trailing slash; the slug is known-safe (callers gate on
/// [`is_workspace_slug`]), so nothing needs escaping.
pub(crate) fn workspace_url(origin: &str, workspace: &str) -> String {
    format!("{origin}/{workspace}/")
}

/// A view inside a workspace a row activation can ask to land on.
///
/// The page names one, and this composes the fragment. It never takes the
/// fragment itself. Every `window-*` webview holds the full IPC permission set
/// on the gateway origin (ADR 0028). A page-supplied URL part would therefore
/// let the page choose what loads in a window carrying that grant. Same
/// reasoning [`is_workspace_slug`] exists for.
///
/// `utils/workspaceLanding.ts` is the frontend mirror, and writes the same
/// fragment for the two mechanisms that never reach this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceLanding {
    /// That workspace's notifications view, from the Lucidos menu's
    /// notifications group.
    Notifications,
}

impl WorkspaceLanding {
    /// The landing a page named, or `None` for a name this build does not
    /// serve. Exact match: no trimming and no case folding, so a caller that
    /// drifted is refused rather than guessed at.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "notifications" => Some(Self::Notifications),
            _ => None,
        }
    }

    /// The URL fragment this landing is delivered as, leading `#` included.
    fn fragment(self) -> &'static str {
        match self {
            Self::Notifications => "#notifications",
        }
    }
}

/// What to append to a workspace URL for `landing`, or `""` for no landing.
fn landing_fragment(landing: Option<WorkspaceLanding>) -> &'static str {
    landing.map_or("", WorkspaceLanding::fragment)
}

/// What an app window's current URL says about where it is pointed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowContext<'a> {
    /// Not pointed at the gateway yet: the declared `main` window still on the
    /// bundled `tauri://localhost` before `desktop::launch` navigates it.
    Unnavigated,
    /// On the gateway but inside no workspace: the picker (`/~/…`) or the bare
    /// root the picker is reached through. Not a workspace, so reusing it steps
    /// on nobody.
    Neutral,
    /// Serving this workspace (`/<slug>/…`).
    Workspace(&'a str),
}

/// Classify a window by its URL.
pub(crate) fn window_context(url: &str) -> WindowContext<'_> {
    if let Some(workspace) = window_workspace(url) {
        return WindowContext::Workspace(workspace);
    }
    match strip_http_scheme(url) {
        // On the gateway but inside no workspace: the root, the `~` sigil, or a
        // path the gateway would not resolve to a workspace at all.
        Some(_) => WindowContext::Neutral,
        None => WindowContext::Unnavigated,
    }
}

/// The origin a window this client opens should be served off: the CALLING
/// window's own, falling back to this install's stable gateway URL.
///
/// The caller's origin is preferred because that is what the web path does.
/// Both workspace lists reach a workspace as the origin-relative `/<slug>/`.
/// So a client reached over a tailnet address opens its next window there too,
/// and a dev window on the vite port stays put. The fallback covers a caller
/// on no http(s) URL at all, i.e. the bundled asset scheme before
/// `desktop::launch` has navigated it.
pub(crate) fn target_origin<'a>(caller_url: Option<&'a str>, fallback: &'a str) -> &'a str {
    caller_url.and_then(window_origin).unwrap_or(fallback)
}

/// The labels of the windows currently in `want`.
///
/// Every caller passes the result straight to [`preferred_label`], which is why
/// the two live together. A chooser filtering by hand would be free to filter
/// differently from the one next to it.
pub(crate) fn labels_in<'a>(
    windows: &'a [(String, String)],
    want: WindowContext<'_>,
) -> Vec<&'a str> {
    windows
        .iter()
        .filter(|(_, url)| window_context(url) == want)
        .map(|(label, _)| label.as_str())
        .collect()
}

/// The window a caller prefers among equally eligible ones.
///
/// `main` wins when it is in the running: it is the window a trayed client
/// hides, so reusing it is what the user sees come back. Otherwise the lowest
/// label wins, which keeps the choice deterministic rather than dependent on
/// map iteration order.
pub(crate) fn preferred_label<'a>(labels: &[&'a str]) -> Option<&'a str> {
    if labels.contains(&crate::app_window::MAIN_WINDOW_LABEL) {
        return Some(crate::app_window::MAIN_WINDOW_LABEL);
    }
    labels.iter().min().copied()
}

/// Where a workspace-row activation should land, on the packaged desktop client.
///
/// Deliberately a different shape from [`crate::notifications::TapTarget`],
/// which answers the same question for a native banner. A tap has no calling
/// window and may aim the boot navigation; a click has one and never may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceTarget {
    /// A window is already on this workspace: show and focus it.
    Focus(String),
    /// The calling window is on no workspace (the picker): point it at `url`.
    Navigate { label: String, url: String },
    /// Nothing here can take it without cost: open a fresh window at `url`.
    NewWindow { url: String },
}

/// Pick the window a click on `workspace`'s row should land in. Takes every
/// top-level app window as `(label, url)`, plus the calling window's label.
///
/// `None` for a slug the gateway would not serve. The page supplies that slug.
/// So the refusal is the whole gate on what can load in a window holding the
/// `window-*` IPC grant (ADR 0028).
///
/// Priority:
///  1. a window already on `workspace` → focus it, so the window count stays
///     bounded by the workspace count;
///  2. the CALLER on no workspace → point it there, so a click in the picker
///     leaves no stray picker window behind;
///  3. otherwise → a new window.
///
/// Step 2 asks only about the CALLER, where a tap takes any neutral window. A
/// tap has nowhere else to go. A click came from a window the user is looking
/// at, and repointing another one they can see is a surprise. An unnavigated
/// window matches neither step, which is deliberate: `desktop::launch` will
/// navigate `main` itself and clobber anything aimed at it.
///
/// A `landing` turns step 1 into a navigation, since only a navigation carries
/// a fragment into a page already loaded. The URL differs from that window's
/// own by the fragment alone, so it keeps its state and answers with a
/// `hashchange`. Without a landing the focus stands: that window may be deep in
/// a thread, and repointing it at the workspace root would throw that away.
pub(crate) fn choose_workspace_target(
    windows: &[(String, String)],
    caller_label: &str,
    workspace: &str,
    origin: &str,
    landing: Option<WorkspaceLanding>,
) -> Option<WorkspaceTarget> {
    if !is_workspace_slug(workspace) {
        return None;
    }
    let url = format!(
        "{}{}",
        workspace_url(origin, workspace),
        landing_fragment(landing)
    );

    let on_workspace = labels_in(windows, WindowContext::Workspace(workspace));
    if let Some(label) = preferred_label(&on_workspace) {
        return Some(match landing {
            Some(_) => WorkspaceTarget::Navigate {
                label: label.to_string(),
                url,
            },
            None => WorkspaceTarget::Focus(label.to_string()),
        });
    }

    let caller_is_neutral = windows
        .iter()
        .any(|(label, url)| label == caller_label && window_context(url) == WindowContext::Neutral);
    if caller_is_neutral {
        return Some(WorkspaceTarget::Navigate {
            label: caller_label.to_string(),
            url,
        });
    }

    Some(WorkspaceTarget::NewWindow { url })
}

#[cfg(test)]
mod tests {
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
        // The picker and the gateway root are not workspaces, so a caller may
        // reuse them without stepping on anything.
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
        // Not a slug the gateway would resolve to a workspace.
        assert_eq!(
            window_context("http://localhost:3210/Not_A_Slug/"),
            WindowContext::Neutral
        );
        // The bundled app URL: `desktop::launch` has not navigated this window yet.
        assert_eq!(
            window_context("tauri://localhost"),
            WindowContext::Unnavigated
        );
        assert_eq!(window_context(""), WindowContext::Unnavigated);
    }

    #[test]
    fn window_origin_keeps_the_authority_and_refuses_a_non_http_url() {
        // No path, https, and a bracketed IPv6 authority all keep their port.
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
        // A scheme with no authority at all is not an origin.
        assert_eq!(window_origin("http://"), None);
        // The bundled app URL contributes none, which is what sends every caller
        // to its own fallback rather than to the literal "null".
        assert_eq!(window_origin("tauri://localhost"), None);
        assert_eq!(window_origin(""), None);
    }

    #[test]
    fn window_is_navigated_splits_the_boot_splash_from_the_picker() {
        assert!(!window_is_navigated("tauri://localhost"));
        assert!(!window_is_navigated(""));
        // The picker IS navigated: the user left a window there on purpose.
        assert!(window_is_navigated("http://localhost:3210/~/"));
        assert!(window_is_navigated("http://localhost:3210/myws/"));
    }

    #[test]
    fn is_workspace_slug_matches_what_the_gateway_would_serve() {
        assert!(is_workspace_slug("myws"));
        assert!(is_workspace_slug("my-ws-2"));
        // The gateway's reserved sigil fails on the character class alone.
        assert!(!is_workspace_slug("~"));
        assert!(!is_workspace_slug(""));
        assert!(!is_workspace_slug("-ws"));
        assert!(!is_workspace_slug("ws-"));
        assert!(!is_workspace_slug("my--ws"));
        assert!(!is_workspace_slug("MyWs"));
        assert!(!is_workspace_slug("my ws"));
        assert!(!is_workspace_slug("my/ws"));
        assert!(!is_workspace_slug(".."));
    }

    #[test]
    fn workspace_url_composes_the_origin_and_the_slug() {
        assert_eq!(
            workspace_url("http://localhost:3210", "myws"),
            "http://localhost:3210/myws/"
        );
    }

    /// `(label, url)` pairs in the shape the choosers take.
    fn windows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(label, url)| ((*label).to_string(), (*url).to_string()))
            .collect()
    }

    const ORIGIN: &str = "http://localhost:3210";
    const FALLBACK: &str = "http://localhost:3210";

    #[test]
    fn a_row_click_focuses_the_window_already_on_that_workspace() {
        // The whole point: the window count stays bounded by the workspace
        // count, so clicking a peer twice never yields two windows on it.
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/work/#thread=t1"),
        ]);
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
        // Including when the caller IS that window: focusing itself is a no-op,
        // and it must never fall through to opening a second one.
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
        // Without `main`, the lowest label, rather than whatever order the
        // window map happened to yield.
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
        // Spawning a window here would leave the picker sitting behind it, and
        // one more stray picker window on every workspace the user opens.
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
        // Where a tap would take any neutral window, a click takes only its own.
        // The user is looking at `window-1`, and repointing `main` behind it
        // would move a window they never touched.
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
    fn an_unnavigated_window_is_never_the_target() {
        // `desktop::launch` is waiting on the gateway and will navigate `main`
        // itself, so anything pointed there is clobbered. The tap path spends a
        // whole variant on aiming that navigation; a click must not touch it.
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
        // And an unnavigated CALLER is not a neutral window either.
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
        // With no windows at all, the same answer.
        assert_eq!(
            choose_workspace_target(&[], "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/".to_string(),
            })
        );
    }

    /// The second window lands on the origin the CALLING window is already on,
    /// which is what makes this match the web path's origin-relative `/<slug>/`.
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
        // Reached over a tailnet address, so the second window goes there too:
        // sending it to loopback would open a window the user cannot use.
        assert_eq!(
            at(Some("https://box.tailnet.ts.net/dev/")),
            Some(WorkspaceTarget::NewWindow {
                url: "https://box.tailnet.ts.net/work/".to_string(),
            })
        );
        // A dev window on the vite port stays on it.
        assert_eq!(
            at(Some("http://localhost:5173/~/")),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:5173/work/".to_string(),
            })
        );
        // A caller on no http(s) origin at all: the bundled asset scheme, before
        // `desktop::launch` has navigated the window. `tauri::Url::origin()`
        // would answer the literal "null" there, hence the fallback.
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

    /// The page supplies the slug. So this refusal is the whole gate on what can
    /// load in a window carrying the `window-*` IPC grant (ADR 0028).
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

    /// The page names the landing. So this is the whole gate on the fragment
    /// half of a URL loaded under the `window-*` IPC grant (ADR 0028).
    #[test]
    fn a_landing_that_is_not_a_name_this_build_serves_is_refused() {
        assert_eq!(
            WorkspaceLanding::parse("notifications"),
            Some(WorkspaceLanding::Notifications)
        );
        for bad in [
            "",
            // The name, not the fragment: the fragment is ours to compose.
            "#notifications",
            // A prefix of the real name, which is the collision the hash
            // router is anchored against on the receiving side.
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

    /// The literal the frontend's `utils/workspaceLanding.ts` also writes, for
    /// the in-place navigation and the browser tab. Those two never reach this
    /// process, so nothing but a pair of tests holds the two in step.
    #[test]
    fn the_notifications_landing_is_the_bare_hash_the_page_router_reads() {
        assert_eq!(WorkspaceLanding::Notifications.fragment(), "#notifications");
        assert_eq!(landing_fragment(None), "");
    }

    #[test]
    fn a_landing_rides_every_target_the_chooser_can_pick() {
        const LANDING: Option<WorkspaceLanding> = Some(WorkspaceLanding::Notifications);
        // A new window.
        assert_eq!(
            choose_workspace_target(&[], "main", "work", ORIGIN, LANDING),
            Some(WorkspaceTarget::NewWindow {
                url: "http://localhost:3210/work/#notifications".to_string(),
            })
        );
        // The picker window this click came from.
        let picker = windows(&[("main", "http://localhost:3210/~/")]);
        assert_eq!(
            choose_workspace_target(&picker, "main", "work", ORIGIN, LANDING),
            Some(WorkspaceTarget::Navigate {
                label: "main".to_string(),
                url: "http://localhost:3210/work/#notifications".to_string(),
            })
        );
    }

    /// A landing has to REACH the page, and a focus carries nothing. So the
    /// window already on the workspace is navigated instead, to a URL differing
    /// from its own by the fragment alone.
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
        // And the window count stays bounded by the workspace count: never a
        // second window on a workspace one is already showing.
        assert_eq!(
            choose_workspace_target(&open, "main", "work", ORIGIN, None),
            Some(WorkspaceTarget::Focus("window-1".to_string()))
        );
    }

    /// A landing is refused for a bad slug like anything else: the slug gate
    /// runs first, so a valid landing cannot smuggle one past it.
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
