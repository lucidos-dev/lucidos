//! The *fix thread*: the one coding-agent thread a red main check opens, and
//! the *flaky fix thread* a pass with flaky tests opens.
//!
//! Each is an ordinary independent thread in the Lucidos repo. Its first
//! prompt carries what failed, where to read the run, and the changes that
//! landed in the range. Its change goes through the landing lane like any
//! other, and when one lands the thread's check closes.

use std::path::Path;

use uuid::Uuid;

use super::state::{CheckRange, FailedTest, FlakyFix, RedCheck};
use crate::engine::successor::Implementer;
use crate::engine::thread_events::EngineReason;
use crate::runtime::CodingAgent;

/// A change that landed in a check's range, for the fix thread's prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandedChange {
    pub(crate) thread_id: Uuid,
    pub(crate) title: String,
}

/// The model a fix thread runs on: a named cause and a short log need no more.
const FIX_THREAD_MODEL: &str = "claude-sonnet-5-5";
const FIX_THREAD_EFFORT: &str = "medium";

/// The implementer a red check's fix thread opens on. The workspace's own
/// default decides nothing here.
pub(crate) fn fix_thread_implementer() -> Implementer {
    Implementer {
        coding_agent: CodingAgent::ClaudeCode,
        model: Some(FIX_THREAD_MODEL.into()),
        reasoning_effort: Some(FIX_THREAD_EFFORT.into()),
    }
}

/// How many characters of the summary a fix thread's title carries.
const TITLE_SUMMARY_CHARS: usize = 60;
/// How many landed changes the prompt lists by name.
const LISTED_CHANGES: usize = 30;

/// The fix thread's title: what is red.
pub(crate) fn fix_thread_title(summary: &str) -> String {
    format!(
        "Fix main: {}",
        cut_to_title(summary.lines().next().unwrap_or(""))
    )
}

/// The flaky fix thread's title: the first flaky test, and how many more.
pub(crate) fn flaky_fix_thread_title(flaky: &[FailedTest]) -> String {
    let first = flaky.first().map_or("", |test| test.name.as_str());
    let more = match flaky.len() {
        0 | 1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    format!("Fix flaky: {}{more}", cut_to_title(first))
}

fn cut_to_title(line: &str) -> String {
    let line = line.trim();
    let cut = line.floor_char_boundary(TITLE_SUMMARY_CHARS);
    let ellipsis = if cut < line.len() { "…" } else { "" };
    format!("{}{ellipsis}", &line[..cut])
}

/// A fix thread to open: the thread, its first prompt and title, who runs it,
/// and the reason its first message carries.
pub(crate) struct FixThreadOpening {
    pub(crate) thread_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) title: String,
    pub(crate) implementer: Implementer,
    pub(crate) reason: EngineReason,
}

/// The red check's fix thread. `landed` is what landed in the check's range,
/// or `None` when git could not list it.
pub(crate) fn red_opening(
    red: &RedCheck,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) -> FixThreadOpening {
    FixThreadOpening {
        thread_id: red.fix_thread_id,
        prompt: build_fix_thread_prompt(red, landed, workspace),
        title: fix_thread_title(&red.summary),
        implementer: fix_thread_implementer(),
        reason: EngineReason::MainCheckRed,
    }
}

/// The flaky fix thread of a check that passed with flaky tests.
pub(crate) fn flaky_opening(
    flaky: &FlakyFix,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) -> FixThreadOpening {
    FixThreadOpening {
        thread_id: flaky.fix_thread_id,
        prompt: build_flaky_fix_thread_prompt(flaky, landed, workspace),
        title: flaky_fix_thread_title(&flaky.flaky),
        implementer: fix_thread_implementer(),
        reason: EngineReason::MainCheckFlaky,
    }
}

/// The fix thread's first prompt.
pub(crate) fn build_fix_thread_prompt(
    red: &RedCheck,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) -> String {
    let mut prompt = format!(
        "The main check failed on main at {to}. It ran the engine suite and every e2e suite \
         on GitHub against a stripped copy of main.\n\n\
         What failed: {summary}\n",
        to = red.range.to_sha,
        summary = red.summary,
    );
    push_run_url(&mut prompt, red.run_url.as_deref());
    if red.failures.is_empty() {
        if !red.tail.trim().is_empty() {
            prompt.push_str(&format!(
                "\nThe end of the failing output:\n```\n{}\n```\n",
                red.tail.trim_end()
            ));
        }
    } else {
        push_failures(&mut prompt, "The failed tests", &red.failures);
    }
    push_range(&mut prompt, &red.range, landed, workspace);
    prompt.push_str(FIX_RULES);
    prompt.push_str(RED_ENDING);
    prompt
}

/// The flaky fix thread's first prompt: the red prompt, framed as flaky.
pub(crate) fn build_flaky_fix_thread_prompt(
    flaky: &FlakyFix,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) -> String {
    let mut prompt = format!(
        "The main check passed on main at {to}, but {n} test(s) failed once and passed on a \
         retry. They are flaky. Nothing is red, and no check waits on you. The check ran the \
         engine suite and every e2e suite on GitHub against a stripped copy of main.\n",
        to = flaky.range.to_sha,
        n = flaky.flaky.len(),
    );
    push_run_url(&mut prompt, flaky.run_url.as_deref());
    push_failures(&mut prompt, "The flaky tests", &flaky.flaky);
    push_range(&mut prompt, &flaky.range, landed, workspace);
    prompt.push_str(FIX_RULES);
    prompt.push_str(FLAKY_ENDING);
    prompt
}

fn push_run_url(prompt: &mut String, run_url: Option<&str>) {
    if let Some(url) = run_url {
        prompt.push_str(&format!("The run: {url}\n"));
    }
}

/// The range the check covered, and the changes that landed in it.
fn push_range(
    prompt: &mut String,
    range: &CheckRange,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) {
    prompt.push_str(&format!(
        "\nIt covers what landed in {from}..{to}",
        from = range.from_sha,
        to = range.to_sha
    ));
    match landed {
        Some([]) => prompt.push_str(", which names no landed change.\n"),
        Some(changes) => {
            prompt.push_str(":\n");
            for change in changes.iter().take(LISTED_CHANGES) {
                let link = crate::core::store::thread_link(workspace, change.thread_id);
                prompt.push_str(&format!("- [{}]({link})\n", change.title));
            }
            if changes.len() > LISTED_CHANGES {
                prompt.push_str(&format!("- and {} more\n", changes.len() - LISTED_CHANGES));
            }
        }
        None => prompt.push_str(". Git could not list the changes in it.\n"),
    }
}

/// What a fix thread must not do, and what it does instead.
const FIX_RULES: &str =
    "\nThe check runs on Linux, and the failure may be Linux only. Find the cause and \
     fix it on your own branch, with a test that fails without the fix.\n\n\
     A timing failure is a race to find, not a number to raise. Raise a timeout, a retry \
     count or a frame allowance only for a measured reason, and state the measurement in \
     the commit.\n\n\
     Once you know the cause, this is a handoff checkpoint: run `lucidos successor offer \
     --at root-cause` and follow what it prints.\n\n\
     Run `/harden`, then end your turn. The maintainer applies your change. ";
const RED_ENDING: &str = "The red closes when it lands, and the next check covers it.";
const FLAKY_ENDING: &str = "This thread closes when it lands, or when the maintainer \
     archives or deletes it. Adding a retry to a flaky test is no fix.";

/// One entry per test: its suite, its name, and the head of its error.
fn push_failures(prompt: &mut String, heading: &str, tests: &[FailedTest]) {
    prompt.push_str(&format!("\n{heading} ({}):\n", tests.len()));
    for test in tests {
        prompt.push_str(&format!("- {}: {}\n", test.suite, test.name));
        let detail = test.detail.trim();
        if !detail.is_empty() {
            prompt.push_str(&format!("```\n{detail}\n```\n"));
        }
    }
}

/// The changes whose landing merges sit on `main`'s first-parent line in
/// `from..to`, oldest first, once per thread. `None` when git could not say.
pub(crate) async fn landed_changes_in_range(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    from: &str,
    to: &str,
) -> Option<Vec<LandedChange>> {
    let range = format!("{from}..{to}");
    let out = crate::engine::git_ops::git_cmd(&["rev-list", "--first-parent", &range], repo_root)
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let shas: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT e.thread_id, COALESCE(NULLIF(e.payload->>'thread_title', ''), ts.title) \
         FROM ( \
             SELECT DISTINCT ON (thread_id) thread_id, payload, sequence FROM events \
             WHERE event_type = 'ChangeApplied' AND payload->>'landing_merge_sha' = ANY($1) \
             ORDER BY thread_id, sequence \
         ) e \
         LEFT JOIN thread_summaries ts ON ts.thread_id = e.thread_id \
         ORDER BY e.sequence",
    )
    .bind(&shas)
    .fetch_all(pool)
    .await
    .ok()?;
    Some(
        rows.into_iter()
            .map(|(thread_id, title)| LandedChange {
                thread_id,
                title: title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "Untitled thread".into()),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::state::CheckRange;
    use super::*;

    fn red(tail: &str, run_url: Option<&str>) -> RedCheck {
        RedCheck {
            range: CheckRange {
                check_id: Uuid::nil(),
                from_sha: "aaa".into(),
                to_sha: "bbb".into(),
            },
            summary: "Failed: engine, browser chromium.".into(),
            tail: tail.into(),
            failures: Vec::new(),
            run_url: run_url.map(str::to_string),
            fix_thread_id: Uuid::nil(),
        }
    }

    /// The prompt names the failure, the run, the tail, the range, and each
    /// landed change as a thread link.
    #[test]
    fn the_prompt_carries_everything_the_fix_needs() {
        let thread = Uuid::from_bytes([3; 16]);
        let landed = [LandedChange {
            thread_id: thread,
            title: "Fix the parser".into(),
        }];
        let prompt = build_fix_thread_prompt(
            &red(
                "test result: FAILED",
                Some("https://example.invalid/runs/7"),
            ),
            Some(&landed),
            "myws",
        );
        for needle in [
            "Failed: engine, browser chromium.",
            "https://example.invalid/runs/7",
            "test result: FAILED",
            "aaa..bbb",
            &format!("[Fix the parser](thread:myws/{thread})"),
            "/harden",
        ] {
            assert!(prompt.contains(needle), "missing {needle:?} in:\n{prompt}");
        }
    }

    fn failure(suite: &str, name: &str, detail: &str) -> FailedTest {
        FailedTest {
            suite: suite.into(),
            name: name.into(),
            detail: detail.into(),
        }
    }

    /// The failed tests lead, and the raw tail stays out once they parsed.
    #[test]
    fn the_prompt_lists_every_failed_test_in_place_of_the_tail() {
        let mut check = red("totals only", None);
        check.failures = vec![
            failure("engine", "engine::a::b", "panicked at a.rs:1"),
            failure("browser chromium", "e2e/x.spec.ts:3:1 › opens", ""),
        ];
        let prompt = build_fix_thread_prompt(&check, Some(&[]), "myws");
        for needle in [
            "The failed tests (2):",
            "- engine: engine::a::b\n```\npanicked at a.rs:1\n```",
            "- browser chromium: e2e/x.spec.ts:3:1 › opens\n",
        ] {
            assert!(prompt.contains(needle), "missing {needle:?} in:\n{prompt}");
        }
        assert!(!prompt.contains("totals only"));
    }

    /// A timing failure is a race, and the prompt says so.
    #[test]
    fn the_prompt_forbids_the_symptom_fix_and_names_linux() {
        let prompt = build_fix_thread_prompt(&red("", None), Some(&[]), "myws");
        for needle in [
            "runs on Linux",
            "race to find",
            "Raise a timeout, a retry count or a frame allowance only for a measured reason",
        ] {
            assert!(prompt.contains(needle), "missing {needle:?} in:\n{prompt}");
        }
    }

    /// The fix thread's implementer is one the pickers offer, so it cannot
    /// rot into a refusal at the first red.
    #[test]
    fn the_fix_thread_implementer_is_one_the_backend_offers() {
        let implementer = fix_thread_implementer();
        let model = implementer.model.as_deref();
        assert_eq!(
            crate::runtime::validate_coding_agent_model(implementer.coding_agent, model),
            Ok(model.map(str::to_string))
        );
        let effort = implementer.reasoning_effort.as_deref();
        assert_eq!(
            crate::runtime::validate_coding_agent_effort(implementer.coding_agent, model, effort),
            Ok(effort.map(str::to_string))
        );
    }

    #[test]
    fn the_prompt_names_the_handoff_checkpoint() {
        let prompt = build_fix_thread_prompt(&red("", None), Some(&[]), "myws");
        assert!(prompt.contains("handoff checkpoint"));
        assert!(prompt.contains("`lucidos successor offer --at root-cause`"));
    }

    #[test]
    fn the_prompt_says_when_the_range_cannot_be_listed() {
        let prompt = build_fix_thread_prompt(&red("", None), None, "myws");
        assert!(prompt.contains("Git could not list"));
        assert!(!prompt.contains("The run:"));
        assert!(!prompt.contains("```"));
        let prompt = build_fix_thread_prompt(&red("", None), Some(&[]), "myws");
        assert!(prompt.contains("names no landed change"));
    }

    #[test]
    fn the_title_names_what_is_red_and_stays_short() {
        assert_eq!(
            fix_thread_title("Failed: engine."),
            "Fix main: Failed: engine."
        );
        let long = "x".repeat(200);
        let title = fix_thread_title(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() < 80);
    }
}
