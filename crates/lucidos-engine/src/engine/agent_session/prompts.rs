/// Workspace context preamble shared by all Lucidos-repo CC system prompts.
pub(super) fn workspace_preamble(workspace_name: &str) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         When the user refers to threads, events, or data, they mean data in this workspace."
    )
}

/// Process-safety rule shared across CC system prompts that can run bash
/// commands. The pkill prevention applies universally — broad `pkill` from
/// any cwd kills every workspace's engine. The `./scripts/...` alternatives
/// only resolve from the Lucidos source tree, so external-repo prompts pass
/// `false` to omit them.
fn process_safety_rule(include_lucidos_scripts: bool) -> String {
    let scripts = if include_lucidos_scripts {
        " To stop a specific workspace: `./scripts/stop.sh -w <workspace-path>`. \
         To restart for e2e tests: `./scripts/web-dev.sh -w e2e-test -b` (handles its own cleanup)."
    } else {
        ""
    };
    format!(
        "\n\n\
         PROCESS SAFETY: Multiple Lucidos workspaces run concurrently. NEVER use \
         `pkill -f lucidos-engine`, `killall lucidos-engine`, or any broad process kill pattern — \
         these kill ALL workspace engines, not just the one you intend (macOS pkill excludes \
         ancestors, so the calling engine survives while silently killing every other workspace).{scripts} \
         To kill a specific engine: `kill $(cat <workspace>/.lucidos/engine.pid)`."
    )
}

/// Override of CC's hardcoded "Creating pull requests" preamble for prompts that
/// run inside the Lucidos repo. External-repo prompts must NOT include this —
/// PRs are the right workflow there.
const NO_PULL_REQUESTS_RULE: &str = "NO PULL REQUESTS: Lucidos is not a PR-based codebase. \
    Never run `gh pr create`, never `git push` your branch, never tell the user to \
    \"open a PR\" or \"submit a PR\". The engine is the merge mechanism: when the user clicks \
    Apply, your branch lands on main and is pushed to the remote in one step. Override CC's \
    default \"Creating pull requests\" guidance — it does not apply here.";

/// Apply/restart rule shared across Lucidos-repo CC system prompts. The
/// file-type list and the hard ban must stay in sync with
/// `engine::git_ops::files_require_restart` (the truth) and the
/// WaitingBanner button label (the user-visible signal). The regression
/// test `lucidos_prompts_carry_full_apply_restart_rule` documents the
/// failure mode that motivated the hard ban.
const APPLY_RESTART_RULE: &str = "APPLY/RESTART: After your session ends, your commits sit \
    as a pending change in the UI. The user explicitly clicks Apply to merge your branch into \
    main — nothing happens automatically. The button label is \"Apply\" (no restart needed) or \
    \"Apply & Restart\" (restart needed); the engine derives this from the touched files. ANY \
    of these triggers restart: a non-test `.rs` file, `Cargo.toml`, `Cargo.lock`, a `.sql` \
    migration under `migrations/`, an SDK bundle source under `packages/lucidos-sdk/`, or an \
    engine-bundled asset (`crates/lucidos-engine/src/api/sdk_iframe.css`, `sdk_iframe_audio.js`). \
    Frontend-only edits (TypeScript/CSS outside those bundled assets) do NOT trigger restart.\n\n\
    DO NOT comment on restart status in your session summary or anywhere else — do not write \
    \"no restart required\", \"restart required\", \"just a code rebuild\", or any equivalent. \
    The button label is the source of truth and the user already sees it. If your intuition \
    disagrees with the button, your intuition is wrong; the engine's `files_require_restart` \
    check (in `crates/lucidos-engine/src/engine/git_ops.rs`) is authoritative.";

/// Hardening reminder shared across all Lucidos-repo CC system prompts. The
/// /harden skill itself runs the test suites and iterates on failure — keep
/// this text in sync with `.claude/commands/harden.md` Phase 4.5.
const HARDENING_RULE: &str = "HARDENING: Once your implementation is complete and committed, \
    you MUST run `/harden`. No exceptions — even for docs-only, CSS-only, comment-only, or \
    seemingly trivial changes. Do not rationalize skipping it (\"too small to harden\", \
    \"nothing to test\", \"just a wording tweak\"). The skill itself decides what to check: \
    it reviews the diff and runs the test suites for the layers you touched, auto-skipping \
    phases when no relevant layers were touched. The harden marker only exists if you actually \
    invoke `/harden` — without the marker, the user pays the wait when they click Apply.";

/// Encourage CC to ask via the structured `AskUserQuestion` tool — which the
/// Lucidos UI renders as clickable buttons — instead of listing options in
/// plaintext that the user has to retype. Applies to chat-style prompts only;
/// hardening and merge-conflict sessions don't dialogue with the user.
const ASK_USER_QUESTION_RULE: &str =
    "ASKING USERS: When you need an answer from the user — a yes/no decision, picking \
     between approaches, choosing from a small list — use the `AskUserQuestion` tool. \
     The Lucidos UI renders its options as clickable buttons; options listed only in your \
     message text force the user to type their reply instead of clicking. ALWAYS use \
     `AskUserQuestion` for any question with 2-4 discrete answers, including the binary \
     yes/no case. This applies at ANY point in your reply, not just the end — mid-stream \
     checkpoints (\"does the framing look right so far?\", \"is this the right direction \
     before I continue?\", \"should I keep going with approach A?\") and end-of-turn \
     confirmations (\"does this look complete?\", \"did I miss anything?\", \"should I \
     proceed with approach A?\") all become buttons, not plaintext, even when they trail \
     a long markdown answer. The trigger is question-shape (yes/no, A vs B, pick-from-list), \
     not position in the message. If you find yourself typing a question mark and then \
     waiting for the user to answer, stop and route it through `AskUserQuestion`. Reserve \
     plaintext questions for genuinely open-ended ones (\"What name should I use for X?\") \
     where pre-baked options would be guesses.";

/// Permission allowlist rule shared across all CC system prompts.
/// Lucidos passes `--allowedTools` when spawning CC, which overrides settings.json
/// permission rules — so tool allowlist edits MUST go in `~/.lucidos/cc-allowed-tools`.
const PERMISSION_CONFIG_RULE: &str = "\n\n\
    PERMISSION CONFIG: Lucidos passes `--allowedTools` to your CC subprocess. This flag \
    OVERRIDES `~/.claude/settings.json` permission rules — adding a tool to settings.json's \
    `permissions.allow` has NO effect for sessions spawned by Lucidos, and the user will keep \
    seeing the permission prompt. \
    Three ways to remember a granted permission, picked via the buttons on the prompt card: \
    (1) `Always allow Tool(scope)` (narrow) and (2) `Always allow` (broad) append to \
    `~/.lucidos/cc-allowed-tools` (one entry per line, blank lines and `#` comments ignored). \
    The file is read on each subprocess spawn — the next CC session (or `claude_code` tool \
    call) picks it up immediately, no engine restart needed. The currently-running subprocess \
    keeps its frozen `--allowedTools` flag, so a freshly-persisted entry only takes effect on \
    the next session. The compiled-in default lives at \
    `crates/lucidos-engine/src/engine/claude_code.rs` (`DEFAULT_CC_ALLOWED_TOOLS`); editing it \
    only helps fresh installs since existing users keep their seeded file. \
    Bare `Edit`/`Write`/`NotebookEdit` cannot be persisted via the broad button — CC's \
    `acceptEdits` mode routes them through `--permission-prompt-tool` for its protected paths \
    (`.claude/`, `.git/`, which never auto-approve in any mode) and auto-approves them \
    everywhere else, so a bare `Edit` line in `cc-allowed-tools` does nothing useful in either \
    case. The UI hides the broad button for those tools. \
    (3) `Allow <scope> for this thread` (session) records the pattern in the engine's \
    in-memory per-thread allow set — the engine intercepts before CC's gate, so it works for \
    every tool and every path including the CC-protected ones. Lost on engine restart, scoped \
    to one thread.";

/// Build the system prompt for Claude Code worktree sessions.
/// Used by both user-initiated CC sessions and LLM-invoked `claude_code` tool calls.
pub(super) fn worktree_system_prompt(branch_name: &str, workspace_name: &str) -> String {
    format!(
        "{preamble}\n\n\
         WORKTREE CONTEXT: You are running in an isolated git worktree on branch `{branch}`. \
         Your working directory is a complete copy of the Lucidos repository. Your changes are \
         isolated and will be merged back to main automatically when you finish.\n\n\
         ISOLATION RULES: Your worktree is your entire world. ALL file edits, builds, and \
         test runs MUST happen inside your worktree directory (your cwd). Never `cd` to or \
         modify files in the main repository. Never reference absolute paths to the main repo. \
         The scripts (`scripts/web-dev.sh`, `scripts/e2e-browser.sh`, etc.) resolve paths \
         relative to where they live — running them from your worktree uses your worktree's \
         code, which is correct. `cargo build` and `cargo test` from your worktree compile \
         your worktree's source (Cargo resolves from `Cargo.toml` in cwd). \
         If you need to run e2e tests against your changes: build the engine in your worktree \
         (`cargo build -p lucidos-engine`), start the e2e workspace from your worktree \
         (`./scripts/web-dev.sh -w e2e-test -b`), then run tests (`./scripts/e2e.sh` for full \
         API + browser, or `./scripts/e2e-api.sh` / `./scripts/e2e-browser.sh` for one suite). \
         All commands run from your worktree directory.\n\n\
         {apply_restart}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. If you abandoned an approach and took a different one, the old \
         edits may still be in the working tree. Discard them with `git checkout -- <file>`. \
         Only intentional changes should remain — stale uncommitted edits get carried into the \
         pending change and cause confusion when the user reviews it.\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         SESSION SUMMARY: After hardening completes, output a structured summary of what \
         was implemented in this session. List each change with its status (committed, applied, \
         pending). Include file names and brief descriptions. This is the last thing you output \
         before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}{permission_config}",
        preamble = workspace_preamble(workspace_name),
        branch = branch_name,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        apply_restart = APPLY_RESTART_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        process_safety = process_safety_rule(true),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for external repository worktree sessions.
pub(super) fn external_repo_system_prompt(
    repo_name: &str,
    branch_name: &str,
    base_ref: &str,
) -> String {
    format!(
        "REPOSITORY CONTEXT: You are working in an isolated git worktree of the \"{repo_name}\" repository \
         on branch `{branch_name}` (based on {base_ref}).\n\n\
         You have full git access. Create feature branches, push, and create PRs as needed. \
         The user's git credentials and CLI tools (gh, etc.) are available.\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {ask_user_question}\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}{permission_config}",
        ask_user_question = ASK_USER_QUESTION_RULE,
        process_safety = process_safety_rule(false),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for recovered orphaned worktree sessions in external repos.
pub(super) fn external_repo_recovery_system_prompt(repo_name: &str, branch_name: &str) -> String {
    format!(
        "RECOVERED SESSION: You are running in a worktree on branch `{branch}` of the \"{repo}\" \
         repository that was orphaned when the Lucidos engine restarted. A previous Claude Code \
         session was working here but was interrupted.\n\n\
         Your job:\n\
         1. Run `git log --oneline main..HEAD` to see what commits the previous session made\n\
         2. Run `git diff` to check for any uncommitted changes\n\
         3. Understand what the previous session was working on\n\
         4. If the work looks complete or nearly complete, clean up and finish\n\
         5. If the work is incomplete or broken, either finish it or revert the problematic parts\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {ask_user_question}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{permission_config}",
        branch = branch_name,
        repo = repo_name,
        ask_user_question = ASK_USER_QUESTION_RULE,
        process_safety = process_safety_rule(false),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for recovered orphaned worktree sessions.
/// The LLM already has the original thread's message history, so this prompt
/// just explains the restart context and tells it to review and continue.
pub(super) fn recovery_system_prompt(branch_name: &str, workspace_name: &str) -> String {
    format!(
        "{preamble}\n\n\
         RECOVERED SESSION: The Lucidos engine restarted while you were working on branch \
         `{branch}`. Your previous session was interrupted but the worktree is intact.\n\n\
         Review the message history above to understand what you were working on, then:\n\
         1. Run `git log --oneline main..HEAD` to see what commits were made\n\
         2. Run `git diff` to check for uncommitted changes\n\
         3. If the work looks complete, clean up and finish\n\
         4. If incomplete, continue where you left off\n\n\
         {apply_restart}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Discard unintentional changes with `git checkout -- <file>`.\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{permission_config}",
        preamble = workspace_preamble(workspace_name),
        branch = branch_name,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        apply_restart = APPLY_RESTART_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        process_safety = process_safety_rule(true),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for merge conflict resolution sessions.
pub(super) fn conflict_resolution_system_prompt() -> &'static str {
    "MERGE CONFLICT RESOLUTION: You are running in a temporary merge worktree. \
     There are unresolved merge conflicts. Your job is to resolve them.\n\n\
     For each conflicted file:\n\
     1. Read the file to see the conflict markers (<<<<<<< HEAD, =======, >>>>>>> branch)\n\
     2. Understand what both sides intended\n\
     3. Edit the file to combine both changes correctly (remove all conflict markers)\n\
     4. Run `git add <file>` to mark it as resolved\n\n\
     When ALL conflicts are resolved, run `git commit --no-edit` to complete the merge.\n\n\
     If any conflict is ambiguous, ask the user before proceeding.\n\n\
     COMMANDS: Never use /cpa — it is for the main working tree only. \
     Just use `git add` + `git commit` directly.\n\n\
     CRITICAL: Never run `exit` as a bash command."
}

/// Build a merge prompt for CC sessions.
/// `merge_target` is the branch to merge (e.g. "main" or a feature branch name).
/// `context` is an optional prefix (e.g. "You are running in a temporary merge worktree.").
/// `description` is an optional change description appended at the end.
pub(crate) fn build_merge_prompt(
    merge_target: &str,
    context: Option<&str>,
    description: Option<&str>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Your branch needs to be merged with main before it can be applied. ");
    if let Some(ctx) = context {
        prompt.push_str(ctx);
        prompt.push('\n');
    }
    prompt.push_str(&format!(
        "\n\
        Please run the following steps:\n\n\
        1. Run `git merge {} --no-edit` to merge into your branch\n\
        2. If there are merge conflicts, resolve them (read the files, understand both sides, \
           edit to keep both working, `git add` each resolved file, then `git commit --no-edit`)\n\
        3. Run `/harden` to harden the merged code\n\
        4. Run `cargo test -p lucidos-engine` and `cd crates/lucidos-app && npm test` to verify\n\
        5. Fix any test failures before finishing\n\n\
        If any conflict is ambiguous, ask the user before proceeding.",
        merge_target,
    ));
    if let Some(desc) = description {
        prompt.push_str(&format!("\n\nThe change being applied: {}", desc));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens that name Lucidos-only slash commands or Lucidos-source script
    /// paths. None of these resolve from an external repo's cwd, so the
    /// external-repo prompts must not mention them — CC would either hunt for
    /// missing files or invoke unknown commands.
    const LUCIDOS_ONLY_TOKENS: &[&str] = &[
        "/harden",
        "/cpa",
        "./scripts/stop.sh",
        "./scripts/web-dev.sh",
        "./scripts/e2e",
    ];

    fn assert_no_lucidos_only_tokens(prompt: &str, label: &str) {
        for token in LUCIDOS_ONLY_TOKENS {
            assert!(
                !prompt.contains(token),
                "{label} must not mention `{token}` — it does not resolve in external repos",
            );
        }
    }

    #[test]
    fn external_repo_prompt_omits_lucidos_only_tokens() {
        let prompt = external_repo_system_prompt("Acme", "feature/x", "origin/main");
        assert_no_lucidos_only_tokens(&prompt, "external_repo_system_prompt");
        assert!(
            prompt.contains("Acme"),
            "must still name the repo so CC knows where it is",
        );
    }

    #[test]
    fn external_repo_recovery_prompt_omits_lucidos_only_tokens() {
        let prompt = external_repo_recovery_system_prompt("Acme", "feature/x");
        assert_no_lucidos_only_tokens(&prompt, "external_repo_recovery_system_prompt");
    }

    #[test]
    fn external_prompts_keep_pkill_prevention() {
        // Slimmer process-safety rule still has to ban broad pkill — that
        // is the whole reason the rule exists; the script alternatives are
        // just bonus info that doesn't apply outside the Lucidos source.
        let prompt = external_repo_system_prompt("Acme", "feature/x", "origin/main");
        assert!(
            prompt.contains("pkill -f lucidos-engine"),
            "external prompt must still warn against broad pkill",
        );
        assert!(
            prompt.contains("kill $(cat <workspace>/.lucidos/engine.pid)"),
            "external prompt must still tell CC how to kill a specific engine",
        );
    }

    #[test]
    fn chat_style_prompts_nudge_use_of_ask_user_question() {
        let cases: &[(&str, String)] = &[
            (
                "worktree_system_prompt",
                worktree_system_prompt("feature/x", "dev"),
            ),
            (
                "external_repo_system_prompt",
                external_repo_system_prompt("Acme", "feature/x", "origin/main"),
            ),
            (
                "recovery_system_prompt",
                recovery_system_prompt("feature/x", "dev"),
            ),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme", "feature/x"),
            ),
        ];
        for (label, prompt) in cases {
            assert!(
                prompt.contains("AskUserQuestion"),
                "{label} must nudge CC to use AskUserQuestion for choice-shaped questions",
            );
            assert!(
                !prompt.to_lowercase().contains("default to `askuserquestion`"),
                "{label} must keep the AskUserQuestion rule as an unconditional imperative \
                 — softer phrasing (\"default to\") let CC slip back to plaintext for \
                 yes/no questions trailing a long markdown answer",
            );
            assert!(
                prompt.contains("does this look complete"),
                "{label} must include a concrete end-of-turn checkpoint example — the \
                 abstract rule alone wasn't enough; CC needs to see the failure case \
                 spelled out",
            );
            assert!(
                prompt.contains("mid-stream"),
                "{label} must keep the mid-stream concept — end-only examples let CC \
                 keep slipping plaintext yes/no questions in the middle of long answers \
                 (\"does the framing look right so far?\"). Pinning the concept word \
                 (not a specific example phrase) lets future rewrites reword the \
                 examples freely, but fails loudly if mid-stream coverage gets dropped \
                 entirely",
            );
        }
    }

    #[test]
    fn lucidos_worktree_prompt_keeps_harden_and_cpa_guidance() {
        // Don't accidentally strip these from the Lucidos-repo prompt while
        // tightening the external one — `/harden` and `/cpa` are real here.
        let prompt = worktree_system_prompt("feature/x", "dev");
        assert!(prompt.contains("/harden"), "Lucidos prompt must keep /harden guidance");
        assert!(prompt.contains("/cpa"), "Lucidos prompt must keep /cpa guidance");
    }

    /// Both Lucidos-repo prompts must carry the full APPLY/RESTART rule. Past
    /// failure mode: a model paraphrased the file-type list (dropped `.rs`)
    /// and added "No restart required" to its session summary while the UI
    /// button said "Apply & Restart". The rule must (a) name every file type
    /// that triggers restart, so a future edit can't quietly narrow the list,
    /// and (b) ban the specific phrases the model used, so even if it drops
    /// the rule from its mental model it can't write the wrong claim.
    #[test]
    fn lucidos_prompts_carry_full_apply_restart_rule() {
        let cases: &[(&str, String)] = &[
            (
                "worktree_system_prompt",
                worktree_system_prompt("feature/x", "dev"),
            ),
            (
                "recovery_system_prompt",
                recovery_system_prompt("feature/x", "dev"),
            ),
        ];
        // Match `engine::git_ops::files_require_restart`. If you add a
        // new trigger there, add it to APPLY_RESTART_RULE and to this
        // list. Use the same string the rule uses (full paths for the
        // bundled assets) so a paraphrase that drops the path also fails.
        let required_file_types = [
            "`.rs`",
            "`Cargo.toml`",
            "`Cargo.lock`",
            // `.sql` alone would pass even if a paraphrase claimed
            // "any `.sql` file triggers restart"; the engine only
            // checks `.sql` files under `migrations/`. Pin the full
            // qualifier so a paraphrase that drops the scope fails.
            "`.sql` migration under `migrations/`",
            "`packages/lucidos-sdk/`",
            "`crates/lucidos-engine/src/api/sdk_iframe.css`",
            "sdk_iframe_audio.js",
        ];
        let banned_phrases = [
            "\"no restart required\"",
            "\"restart required\"",
            "\"just a code rebuild\"",
        ];
        for (label, prompt) in cases {
            for needle in required_file_types {
                assert!(
                    prompt.contains(needle),
                    "{label} must name `{needle}` (paraphrasing forbidden)",
                );
            }
            for needle in banned_phrases {
                assert!(
                    prompt.contains(needle),
                    "{label} must ban the phrase {needle}",
                );
            }
            assert!(
                prompt.contains("button label is the source of truth"),
                "{label} must name the button as authoritative",
            );
            assert!(
                prompt.contains("files_require_restart"),
                "{label} must point at the engine function (`files_require_restart`)",
            );
        }
    }
}
