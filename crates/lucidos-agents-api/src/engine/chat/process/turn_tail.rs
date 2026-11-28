use crate::engine::engine_version::VersionStatus;
use crate::engine::LucidosEngine;

#[easy_ext::ext(TurnTailExt)]
pub(crate) impl LucidosEngine {
    fn client_url(&self) -> String {
        if let Some(origin) = self.frontend_origin.lock().unwrap().as_ref() {
            return origin.clone();
        }
        let api_port = std::env::var("LUCIDOS_API_PORT").unwrap_or_else(|_| "3000".to_string());
        let port = std::env::var("VITE_PORT").unwrap_or(api_port);
        format!("{}://localhost:{}", crate::net_config::tls_scheme(), port)
    }
}

pub(super) const ENGINE_BUILD_POINTER: &str = "\n\nENGINE BUILD:\n\
     The [ENGINE BUILD] block at the END of the request says whether the user \
     has switched onto the newest build, and is rebuilt every turn. That is \
     the answer to \"has the user restarted?\". Never ask, and never infer it \
     from what you applied earlier: they restart when they like, including \
     mid-turn.";

pub(super) fn engine_build_block(status: &VersionStatus) -> String {
    let state = if status.update_available {
        "A NEWER ENGINE IS BUILT AND THE USER HAS NOT SWITCHED ONTO IT YET, so an applied \
         restart-requiring change is NOT live."
    } else if status.build_deferred.is_some() {
        "A NEW ENGINE VERSION WAITS FOR ACTIVE CODING THREADS TO FINISH. It builds by itself \
         when they finish, or the user can choose Build now. There is nothing to switch onto yet."
    } else if status.source_behind_head {
        "A RESTART-REQUIRING CHANGE IS ON MAIN WITH NO BUILD BEHIND IT YET (rebuilding, or \
         it failed), so there is nothing to switch onto: do not tell them to."
    } else {
        "THE RUNNING ENGINE IS CURRENT, matching both the built binary and main. Any applied \
         restart-requiring change IS live, and the user HAS restarted if one needed it."
    };
    format!("[ENGINE BUILD]\n{state}\n[END ENGINE BUILD]")
}

pub(super) const CLIENT_URL_POINTER: &str = "\n\nThe Lucidos client the user \
     is talking to you from is at the URL in the [CLIENT URL] block at the END \
     of the request. To see an app UI, use capture_app, never browser_open.";

pub(super) fn client_url_block(frontend_url: &str) -> String {
    format!("[CLIENT URL]\n{frontend_url}\n[END CLIENT URL]")
}

#[derive(Default)]
pub(crate) struct TurnTail<'a> {
    pub child_threads: &'a str,
    pub engine_build: &'a str,
    pub client_url: &'a str,
    pub current_time: &'a str,
}

pub(super) fn version_status(update_available: bool, source_behind_head: bool) -> VersionStatus {
    VersionStatus {
        build_deferred: None,
        build_id: "test".to_string(),
        update_available,
        disk_build_id: None,
        packaged: false,
        build_state: "idle",
        source_behind_head,
        head_commit: None,
        build_failure: None,
        shared_build_in_progress: false,
        build_elapsed_ms: None,
        build_queued: None,
        pending_commits: None,
        frontend_refresh_elapsed_ms: None,
        frontend_build_watch_stopped: None,
    }
}

pub(super) fn build_statuses() -> Vec<VersionStatus> {
    let mut statuses: Vec<_> = [(false, false), (true, false), (false, true)]
        .into_iter()
        .map(|(update, behind)| version_status(update, behind))
        .collect();
    let mut held = version_status(false, true);
    held.build_deferred = Some(crate::engine::engine_version::DeferredBuild {
        coding_threads: 1,
        threads: Vec::new(),
        deferred_ms: 0,
    });
    statuses.push(held);
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_current_engine_says_so_affirmatively() {
        let block = engine_build_block(&version_status(false, false));

        assert!(
            block.contains("RUNNING ENGINE IS CURRENT"),
            "must state the current case in the affirmative:\n{block}"
        );
        assert!(
            block.contains("HAS restarted"),
            "must answer the restart question outright:\n{block}"
        );
    }

    #[test]
    fn a_built_but_unswitched_engine_says_the_change_is_not_live() {
        let block = engine_build_block(&version_status(true, false));

        assert!(
            block.contains("HAS NOT SWITCHED ONTO IT YET"),
            "must state that the user has not switched:\n{block}"
        );
        assert!(
            block.contains("is NOT live"),
            "must draw the consequence for an applied change:\n{block}"
        );
    }

    #[test]
    fn source_ahead_of_a_built_binary_does_not_tell_the_user_to_switch() {
        let block = engine_build_block(&version_status(false, true));

        assert!(
            block.contains("NO BUILD BEHIND IT YET"),
            "must distinguish source-ahead from binary-ready:\n{block}"
        );
        assert!(
            block.contains("nothing to switch onto"),
            "must not send the user to a switch that cannot work:\n{block}"
        );
    }

    #[test]
    fn the_build_states_are_distinct() {
        let blocks: Vec<String> = build_statuses()
            .into_iter()
            .map(|status| engine_build_block(&status))
            .collect();

        for (i, a) in blocks.iter().enumerate() {
            for b in &blocks[i + 1..] {
                assert_ne!(a, b, "two build states render the same text");
            }
        }
    }

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

    #[test]
    fn the_block_carries_no_build_id() {
        let mut status = version_status(true, false);
        status.build_id = "deadbeef1".to_string();
        status.disk_build_id = Some("cafebabe2".to_string());

        let block = engine_build_block(&status);

        assert!(
            !block.contains("deadbeef1") && !block.contains("cafebabe2"),
            "a build id is meaningless to the user and must not be quotable:\n{block}"
        );
    }

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

    #[test]
    fn both_tail_blocks_are_delimited() {
        let build = engine_build_block(&version_status(false, false));
        assert!(build.starts_with("[ENGINE BUILD]"), "{build}");
        assert!(build.ends_with("[END ENGINE BUILD]"), "{build}");

        let url = client_url_block("https://localhost:5173");
        assert!(url.starts_with("[CLIENT URL]"), "{url}");
        assert!(url.ends_with("[END CLIENT URL]"), "{url}");
        assert!(url.contains("https://localhost:5173"), "{url}");
    }
}
