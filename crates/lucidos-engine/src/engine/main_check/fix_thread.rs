//! The *fix thread*: the one coding-agent thread a red main check opens.
//!
//! It is an ordinary independent thread in the Lucidos repo. Its first prompt
//! carries what failed, where to read the run, and the changes that landed in
//! the range. Its change goes through the landing lane like any other, and
//! when one lands the red closes.

use std::path::Path;

use uuid::Uuid;

use super::state::RedCheck;

/// A change that landed in a check's range, for the fix thread's prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandedChange {
    pub(crate) thread_id: Uuid,
    pub(crate) title: String,
}

/// How many characters of the summary a fix thread's title carries.
const TITLE_SUMMARY_CHARS: usize = 60;
/// How many landed changes the prompt lists by name.
const LISTED_CHANGES: usize = 30;

/// The fix thread's title: what is red.
pub(crate) fn fix_thread_title(summary: &str) -> String {
    let line = summary.lines().next().unwrap_or("").trim();
    let cut = line.floor_char_boundary(TITLE_SUMMARY_CHARS);
    let ellipsis = if cut < line.len() { "…" } else { "" };
    format!("Fix main: {}{ellipsis}", &line[..cut])
}

/// The fix thread's first prompt. `landed` is what landed in the red check's
/// range, or `None` when git could not list it.
pub(crate) fn build_fix_thread_prompt(
    red: &RedCheck,
    landed: Option<&[LandedChange]>,
    workspace: &str,
) -> String {
    let range = &red.range;
    let mut prompt = format!(
        "The main check failed on main at {to}. It ran the engine suite and every e2e suite \
         on GitHub against a stripped copy of main.\n\n\
         What failed: {summary}\n",
        to = range.to_sha,
        summary = red.summary,
    );
    if let Some(url) = &red.run_url {
        prompt.push_str(&format!("The run: {url}\n"));
    }
    if !red.tail.trim().is_empty() {
        prompt.push_str(&format!(
            "\nThe end of the failing output:\n```\n{}\n```\n",
            red.tail.trim_end()
        ));
    }
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
    prompt.push_str(
        "\nFind the cause and fix it on your own branch, with a test that fails without the \
         fix. Run `/harden`, then end your turn. The maintainer applies your change. The \
         red closes when it lands, and the next check covers it.",
    );
    prompt
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
