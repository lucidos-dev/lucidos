//! Two more values that must not sit in the cached system block (ADR 0084),
//! split the way [`super::turn_clock`] split the clock.
//!
//! The rule the system block lives by is that it holds nothing varying per
//! turn or per thread: it is a function of workspace state and preferences,
//! and of nothing else. Two values broke it, and neither tripped the guard,
//! because that guard compares two threads at ONE moment and both read the
//! same current value.
//!
//! - **The engine build state**, a function of live build state, rebuilt
//!   every turn. Its four states are 327 to 374 characters, and the 7-day
//!   data has two threads on one build disagreeing by exactly 44 of them.
//! - **The client URL**, read from `frontend_origin`, a runtime value set
//!   from the last observed request origin.
//!
//! Each splits the same way. The invariant guidance is a `const` taking no
//! argument, so nothing volatile can reach the cached tier through it. The
//! reading rides in a labelled block at the end of the user message.

use crate::engine::engine_version::VersionStatus;
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// The origin the user is reaching this workspace on, read once per turn.
    ///
    /// `frontend_origin` is set from the last observed request, so this is a
    /// runtime reading and not workspace state. That is exactly why it feeds
    /// [`client_url_block`] rather than the system prompt.
    pub(super) fn client_url(&self) -> String {
        if let Some(origin) = self.frontend_origin.lock().unwrap().as_ref() {
            return origin.clone();
        }
        // No request origin observed yet: the engine serves the frontend
        // itself, so its own TLS setting decides the scheme (never hardcode
        // http/https, see the intra-host scheme rule).
        let api_port = std::env::var("LUCIDOS_API_PORT").unwrap_or_else(|_| "3000".to_string());
        let port = std::env::var("VITE_PORT").unwrap_or(api_port);
        format!("{}://localhost:{}", crate::net_config::tls_scheme(), port)
    }
}

/// The invariant half of the engine build state, spliced into the cached
/// system block. Dev installs only, matching the apply-verify addendum that
/// forwards to it.
///
/// Every claim here holds on every turn: where the reading is, what question
/// it settles, and that it may move under the agent mid-turn. The state
/// itself is the one thing that changes, and that is [`engine_build_block`].
pub(super) const ENGINE_BUILD_POINTER: &str = "\n\nENGINE BUILD:\n\
     The [ENGINE BUILD] block at the END of the request says whether the user \
     has switched onto the newest build, and is rebuilt every turn. That is \
     the answer to \"has the user restarted?\". Never ask, and never infer it \
     from what you applied earlier: they restart when they like, including \
     mid-turn.";

/// The volatile half: what the running engine is right now.
///
/// The four states are not the same claim, so none of them may collapse into
/// another. Source-ahead is split from wedged because the advice inverts:
/// source-ahead resolves by waiting, while a wedged rebuild resolves only by
/// relaunching, and the agent must not send the user round that loop.
///
/// Deliberately no build id in the text. The user cannot look one up on any
/// screen, so it would only invite the agent to quote a hex string at them.
/// See `.claude/rules/glossary.md` and the prompt's own NAMES NOT IDS rule.
pub(super) fn engine_build_block(status: &VersionStatus) -> String {
    let state = if status.update_available {
        "A NEWER ENGINE IS BUILT AND THE USER HAS NOT SWITCHED ONTO IT YET, so an applied \
         restart-requiring change is NOT live."
    } else if status.rebuild_wedged {
        "NO REBUILD CAN DELIVER THE RESTART-REQUIRING CHANGE ON MAIN: one already succeeded and \
         produced nothing switchable. Do not offer Switch or Rebuild; relaunch instead."
    } else if status.source_behind_head {
        "A RESTART-REQUIRING CHANGE IS ON MAIN WITH NO BUILD BEHIND IT YET (rebuilding, or \
         it failed), so there is nothing to switch onto: do not tell them to."
    } else {
        "THE RUNNING ENGINE IS CURRENT, matching both the built binary and main. Any applied \
         restart-requiring change IS live, and the user HAS restarted if one needed it."
    };
    format!("[ENGINE BUILD]\n{state}\n[END ENGINE BUILD]")
}

/// The invariant half of the client URL: what the agent should do about it.
///
/// The URL itself is a runtime value, so only the pointer is cacheable.
pub(super) const CLIENT_URL_POINTER: &str = "\n\nThe Lucidos client the user \
     is talking to you from is at the URL in the [CLIENT URL] block at the END \
     of the request. To see an app UI, use capture_app, never browser_open.";

/// The volatile half: the origin this workspace was last reached on.
pub(super) fn client_url_block(frontend_url: &str) -> String {
    format!("[CLIENT URL]\n{frontend_url}\n[END CLIENT URL]")
}

/// The blocks that ride at the END of the user message rather than in the
/// cached system block, in wire order.
///
/// Named rather than three positional `&str`s. The consumers pass them
/// through a long argument list, where two adjacent strings are one typo away
/// from swapping.
pub(crate) struct TurnTail<'a> {
    pub engine_build: &'a str,
    pub client_url: &'a str,
    pub current_time: &'a str,
}

/// The three flags [`engine_build_block`] branches on, with the rest of
/// `VersionStatus` inert. Shared with the always-loaded budget meter, which
/// bills the widest of the four states.
#[cfg(test)]
pub(super) fn version_status(
    update_available: bool,
    source_behind_head: bool,
    rebuild_wedged: bool,
) -> VersionStatus {
    VersionStatus {
        build_id: "test".to_string(),
        update_available,
        disk_build_id: None,
        packaged: false,
        build_state: "idle",
        source_behind_head,
        head_commit: None,
        rebuild_wedged,
        build_failure: None,
        shared_build_in_progress: false,
        build_elapsed_ms: None,
        pending_commits: None,
    }
}

/// The four states, as the flag triples that select them.
#[cfg(test)]
pub(super) const BUILD_STATES: [(bool, bool, bool); 4] = [
    (false, false, false),
    (true, false, false),
    (false, true, false),
    (false, true, true),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing pending, so an applied restart-requiring change IS live and the
    /// user HAS restarted. Stated affirmatively, because "no update available"
    /// is a double negative the model has to reason through at exactly the
    /// moment it is guessing.
    #[test]
    fn a_current_engine_says_so_affirmatively() {
        let block = engine_build_block(&version_status(false, false, false));

        assert!(
            block.contains("RUNNING ENGINE IS CURRENT"),
            "must state the current case in the affirmative:\n{block}"
        );
        assert!(
            block.contains("HAS restarted"),
            "must answer the restart question outright:\n{block}"
        );
    }

    /// A built-but-not-switched engine is the one case where "the user has not
    /// restarted" is true, and it is the only case that may say so.
    #[test]
    fn a_built_but_unswitched_engine_says_the_change_is_not_live() {
        let block = engine_build_block(&version_status(true, false, false));

        assert!(
            block.contains("HAS NOT SWITCHED ONTO IT YET"),
            "must state that the user has not switched:\n{block}"
        );
        assert!(
            block.contains("is NOT live"),
            "must draw the consequence for an applied change:\n{block}"
        );
    }

    /// Source ahead with no binary behind it is NOT "the user has not
    /// switched": there is nothing to switch onto, so telling them to restart
    /// sends them to a button that would do nothing.
    #[test]
    fn source_ahead_of_a_built_binary_does_not_tell_the_user_to_switch() {
        let block = engine_build_block(&version_status(false, true, false));

        assert!(
            block.contains("NO BUILD BEHIND IT YET"),
            "must distinguish source-ahead from binary-ready:\n{block}"
        );
        assert!(
            block.contains("nothing to switch onto"),
            "must not send the user to a switch that cannot work:\n{block}"
        );
    }

    /// A wedged rebuild is source-ahead with the advice inverted. Waiting for
    /// a build and pressing Rebuild are both dead ends. The block must say so,
    /// rather than reuse the "rebuilding, or it failed" wording that invites
    /// both.
    #[test]
    fn a_wedged_rebuild_does_not_send_the_user_round_the_loop() {
        let block = engine_build_block(&version_status(false, true, true));

        assert!(
            block.contains("NO REBUILD CAN DELIVER"),
            "must state that rebuilding is futile:\n{block}"
        );
        assert!(
            !block.contains("NO BUILD BEHIND IT YET"),
            "must not fall through to the retryable source-ahead wording:\n{block}"
        );
        assert!(
            block.contains("relaunch instead"),
            "must name the one thing that does resolve it:\n{block}"
        );
    }

    /// The four cases must be genuinely different text. Collapsing any two
    /// would restore the guess this block exists to remove.
    #[test]
    fn the_four_build_states_are_distinct() {
        let blocks: Vec<String> = BUILD_STATES
            .into_iter()
            .map(|(update, behind, wedged)| {
                engine_build_block(&version_status(update, behind, wedged))
            })
            .collect();

        for (i, a) in blocks.iter().enumerate() {
            for b in &blocks[i + 1..] {
                assert_ne!(a, b, "two build states render the same text");
            }
        }
    }

    /// It answers the question the apply-verify addendum forwards to it, and
    /// says the answer can change under the agent's feet: the user restarts on
    /// their own schedule, including while a turn is running.
    ///
    /// Both claims are invariant, so the POINTER carries them. The addendum
    /// names "ENGINE BUILD section", which is the pointer's own heading.
    #[test]
    fn the_pointer_answers_the_restart_question_and_dates_itself() {
        assert!(
            ENGINE_BUILD_POINTER.contains("has the user restarted?"),
            "must name the question it settles:\n{ENGINE_BUILD_POINTER}"
        );
        assert!(
            ENGINE_BUILD_POINTER.contains("rebuilt every turn"),
            "must say the reading is fresh, or a long thread treats it as \
             stale:\n{ENGINE_BUILD_POINTER}"
        );
        assert!(
            ENGINE_BUILD_POINTER.contains("mid-turn"),
            "must warn that the answer can change during a turn:\n{ENGINE_BUILD_POINTER}"
        );
        assert!(
            ENGINE_BUILD_POINTER.contains("[ENGINE BUILD] block at the END"),
            "must say where the reading actually is:\n{ENGINE_BUILD_POINTER}"
        );
    }

    /// No build id in the text. The user cannot look one up on any screen, so
    /// putting it here only invites the agent to quote a hex string at them.
    #[test]
    fn the_block_carries_no_build_id() {
        let mut status = version_status(true, false, false);
        status.build_id = "deadbeef1".to_string();
        status.disk_build_id = Some("cafebabe2".to_string());

        let block = engine_build_block(&status);

        assert!(
            !block.contains("deadbeef1") && !block.contains("cafebabe2"),
            "a build id is meaningless to the user and must not be quotable:\n{block}"
        );
    }

    /// The whole point of the split: nothing a build state says may appear in
    /// the cached half, or the system tier is rewritten when the state flips.
    #[test]
    fn the_cached_pointer_leaks_no_build_state_and_no_url() {
        for leaked in [
            "RUNNING ENGINE IS CURRENT",
            "HAS NOT SWITCHED ONTO IT YET",
            "NO BUILD BEHIND IT YET",
            "NO REBUILD CAN DELIVER",
            "localhost",
            "http",
        ] {
            assert!(
                !ENGINE_BUILD_POINTER.contains(leaked),
                "the cached engine-build pointer leaks {leaked}:\n{ENGINE_BUILD_POINTER}"
            );
            assert!(
                !CLIENT_URL_POINTER.contains(leaked),
                "the cached client-URL pointer leaks {leaked}:\n{CLIENT_URL_POINTER}"
            );
        }
    }

    /// The client URL pointer keeps the instruction that rode with the URL. It
    /// is the reason the sentence exists at all: an agent that reaches for
    /// browser_open on its own app UI gets a screenshot of nothing useful.
    #[test]
    fn the_client_url_pointer_keeps_the_capture_app_instruction() {
        assert!(
            CLIENT_URL_POINTER.contains("use capture_app, never browser_open"),
            "the instruction must survive the split:\n{CLIENT_URL_POINTER}"
        );
        assert!(
            CLIENT_URL_POINTER.contains("[CLIENT URL] block at the END"),
            "must say where the URL actually is:\n{CLIENT_URL_POINTER}"
        );
    }

    /// Both tail blocks are labelled and closed, so the model can tell where
    /// each ends inside one concatenated user message.
    #[test]
    fn both_tail_blocks_are_delimited() {
        let build = engine_build_block(&version_status(false, false, false));
        assert!(build.starts_with("[ENGINE BUILD]"), "{build}");
        assert!(build.ends_with("[END ENGINE BUILD]"), "{build}");

        let url = client_url_block("https://localhost:5173");
        assert!(url.starts_with("[CLIENT URL]"), "{url}");
        assert!(url.ends_with("[END CLIENT URL]"), "{url}");
        assert!(url.contains("https://localhost:5173"), "{url}");
    }
}
