//! Prompts for applying a change or resolving a landing lane.

/// Build a merge prompt for coding-agent sessions.
/// `merge_target` is the branch to merge (e.g. "main" or a feature branch name).
/// `context` is an optional prefix (e.g. "You are running in a temporary merge worktree.").
/// `description` is an optional change description appended at the end.
pub fn build_merge_prompt(
    merge_target: &str,
    context: Option<&str>,
    description: Option<&str>,
    is_app: bool,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Your branch needs to be merged with main before it can be applied. ");
    if let Some(ctx) = context {
        prompt.push_str(ctx);
        prompt.push('\n');
    }
    // App coding-agent threads own their hardening and lack the Lucidos test
    // tooling. The source-repo prompt asks for `/harden`, which selects the
    // relevant suites.
    if is_app {
        prompt.push_str(&format!(
            "\n\
            Please run the following steps:\n\n\
            1. Run `git merge {} --no-edit` to merge into your branch\n\
            2. If there are merge conflicts, resolve them (read the files, understand both sides, \
               edit to keep both working, `git add` each resolved file, then `git commit --no-edit`)\n\
            3. Do a quick bug-check pass on the merged result, and run the app's own test/lint \
               commands if it ships any\n\n\
            If any conflict is ambiguous, ask the user before proceeding.",
            merge_target,
        ));
    } else {
        prompt.push_str(&format!(
            "\n\
            Please run the following steps:\n\n\
            1. Run `git merge {} --no-edit` to merge into your branch\n\
            2. If there are merge conflicts, resolve them (read the files, understand both sides, \
               edit to keep both working, `git add` each resolved file, then `git commit --no-edit`)\n\
            3. Run `/harden` to harden the merged code (it runs the test suites the change needs)\n\
            4. Fix any failures before finishing\n\n\
            If any conflict is ambiguous, ask the user before proceeding.",
            merge_target,
        ));
    }
    if let Some(desc) = description {
        prompt.push_str(&format!("\n\nThe change being applied: {}", desc));
    }
    prompt
}

/// What a lane thread's prompt says about one member of its batch.
pub struct LaneMemberBrief<'a> {
    /// The member's thread title. It names the landing merge.
    pub title: &'a str,
    /// What the change does, from its description.
    pub summary: &'a str,
    pub branch: &'a str,
    /// The exact commit the lane lands for it.
    pub snapshot_sha: &'a str,
}

/// Longest member summary a lane prompt quotes. The thread title already says
/// what the change is, so the summary only adds intent.
const LANE_SUMMARY_CHARS: usize = 400;

/// The first prompt of a lane thread: one landing merge per member, in lane
/// order, then one `/harden`.
pub fn build_lane_thread_prompt(members: &[LaneMemberBrief<'_>]) -> String {
    let mut prompt = match members.len() {
        1 => "This change would not merge cleanly onto main, so it lands through a lane \
              batch. The integration branch starts at main. Merge it:\n"
            .to_string(),
        n => format!(
            "These {n} changes would not merge cleanly onto main, so they land together as \
             one lane batch. The integration branch starts at main. Merge them in this order:\n"
        ),
    };
    for (i, member) in members.iter().enumerate() {
        let message = shell_single_quote(&format!("Land: {}", member.title));
        prompt.push_str(&format!(
            "\n{}. {} (branch `{}`)\n   `git merge --no-ff {} -m {}`\n",
            i + 1,
            member.title,
            member.branch,
            member.snapshot_sha,
            message
        ));
        let summary = member.summary.split("\n\n").next().unwrap_or("").trim();
        if !summary.is_empty() {
            let cut = summary.floor_char_boundary(LANE_SUMMARY_CHARS);
            prompt.push_str(&format!("   What it does: {}\n", &summary[..cut]));
        }
    }
    prompt.push_str(
        "\nThen:\n\
         1. Resolve each conflict inside the merge that raised it, then `git commit --no-edit`.\n\
         2. Run `/harden` once, after the last merge. It reviews your resolutions and runs \
            the suites the merged result needs.\n\
         3. Ask the user with the question tool when a resolution needs a judgment.\n\
         4. Never rebase, and never touch a source thread's worktree.\n\n\
         End your turn when everything is merged and hardened. The batch lands then.",
    );
    prompt
}

/// How many of `main`'s new commits a catch-up prompt lists.
const LANE_CATCH_UP_COMMITS: usize = 20;

/// The follow-up turn a lane thread gets when `main` moved while it resolved,
/// and the catch-up merge conflicted with its resolution. `commits` are
/// one-line summaries of what `main` gained.
pub fn build_lane_catch_up_prompt(main_sha: &str, commits: &[String]) -> String {
    let mut prompt =
        "main gained these commits while you resolved, and they conflict with the batch:\n"
            .to_string();
    for commit in commits.iter().take(LANE_CATCH_UP_COMMITS) {
        prompt.push_str(&format!("- {commit}\n"));
    }
    if commits.len() > LANE_CATCH_UP_COMMITS {
        prompt.push_str(&format!(
            "- and {} more\n",
            commits.len() - LANE_CATCH_UP_COMMITS
        ));
    }
    prompt.push_str(&format!(
        "\nMerge main with `git merge --no-ff {main_sha} -m 'Catch up with main'`, and resolve \
         the conflicts inside that merge. Run `/harden` again if your resolution changed code. \
         Then end your turn. The batch lands when it ends."
    ));
    prompt
}

/// `text` as one single-quoted shell word.
fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
