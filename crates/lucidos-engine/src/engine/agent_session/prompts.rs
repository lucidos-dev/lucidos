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

/// Tell CC that "Task not found" / "task already completed" after a task ends
/// is expected — the engine evicts the bg-bash registry record on completion,
/// and CC's own task-list entries also get cleaned up. CC was treating these
/// as failures and retrying (nightly workspace-learning flagged 5/day on
/// `TaskOutput`, then 3 `TaskUpdate` in one thread within 1 minute). The rule
/// covers all three lookup tools so a single sighting in any prompt
/// inoculates the model. Applies to chat-style prompts only; merge-conflict
/// sessions don't run bg tasks.
const TASK_LIFECYCLE_RULE: &str = "TASK LIFECYCLE: After a background task ends, its registry \
    record is evicted — so subsequent `TaskOutput`, `TaskUpdate`, or `TaskList` calls referencing \
    that id return errors like \"Task not found\" or \"task already completed\". This is \
    **expected**, not a bug. Treat the error as confirmation the task is done; do NOT retry the \
    call.";

/// Encourage CC to ask via the structured `AskUserQuestion` tool — which the
/// Lucidos UI renders as clickable buttons — instead of listing options in
/// plaintext that the user has to retype. Applies to chat-style prompts only;
/// hardening and merge-conflict sessions don't dialogue with the user.
const ASK_USER_QUESTION_RULE: &str =
    "ASKING USERS: When you need an answer from the user — a yes/no decision, picking \
     between approaches, choosing from a small list — use the `AskUserQuestion` tool. \
     The Lucidos UI renders its options as clickable buttons; options listed only in your \
     message text force the user to type their reply instead of clicking. ALWAYS provide the \
     `question` field — the full question text shown on the card; the optional `header` \
     chip-label is never a substitute, so don't put the question only in `header` (or only \
     in your prose) and leave `question` empty. The engine rejects a call whose `question` \
     is missing and makes you re-ask. ALWAYS use \
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
     where pre-baked options would be guesses. \
     NEVER parallel-call `AskUserQuestion` alongside other tools — if you're asking a \
     question, stop the assistant message after the `AskUserQuestion` tool_use and do not \
     include any sibling tool_uses (no Bash, no Read, no TaskOutput, no second \
     AskUserQuestion). Lucidos's PreToolUse hook blocks `AskUserQuestion` for up to 24h, \
     but any sibling tool_uses in the same message dispatch in parallel and emit progression \
     events while the question is still on-screen — at which point the user's typed comment \
     can no longer be safely routed as a free-text answer, and your own parallel work has \
     wasted tokens on an unconfirmed direction. Wait for the answer, THEN continue.";

/// Permission allowlist rule shared across all CC system prompts.
/// Lucidos passes `--allowedTools` when spawning CC, which overrides settings.json
/// permission rules — so tool allowlist edits MUST go in `~/.lucidos/cc-allowed-tools`.
const PERMISSION_CONFIG_RULE: &str = "\n\n\
    PERMISSION CONFIG: Lucidos passes `--allowedTools` to your Claude Code subprocess. This flag \
    OVERRIDES `~/.claude/settings.json` permission rules — adding a tool to settings.json's \
    `permissions.allow` has NO effect for sessions spawned by Lucidos, and the user will keep \
    seeing the permission prompt. \
    Three ways to remember a granted permission, picked via the buttons on the prompt card: \
    (1) `Always allow Tool(scope)` (narrow) and (2) `Always allow` (broad) append to \
    `~/.lucidos/cc-allowed-tools` (one entry per line, blank lines and `#` comments ignored). \
    The file is read on each subprocess spawn — the next Claude Code session (or `claude_code` tool \
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

/// App-building knowhow pointer shared by the two app worktree prompts
/// (`app_worktree_system_prompt`, `app_worktree_recovery_system_prompt`).
///
/// The engine ships authoritative app-building guides under `system-knowhow/`
/// (`building-an-app`, `js-sdk`, `best-practices`, …) that the chat agent
/// loads via its `load_knowhow` tool. An app coding-agent thread can't reach
/// them: its worktree is a sparse-checkout of the *workspace* git narrowed to
/// one app folder, so the docs are neither on disk nor exposed via that tool.
/// The `lucidos knowhow` CLI subcommand fetches them from the parent engine
/// over HTTP, giving the session the same guidance on demand — without it,
/// app sessions reinvent the SDK surface and repeat the documented mistakes
/// (wrong `artifacts/` data path, hand-rolled proxy URLs, storing data under
/// `apps/<id>/`). Lucidos-source prompts deliberately omit this: that worktree
/// is a full repo checkout with `system-knowhow/` already on disk.
const APP_KNOWHOW_RULE: &str = "APP-BUILDING KNOWHOW: This workspace's engine ships \
    authoritative guides for building Lucidos apps — file layout, the `lucidos.*` SDK \
    surface, data-path rules, the external-API proxy pattern, and common mistakes. They \
    are NOT in this worktree (it is sparse-checkout-narrowed to the app folder), so fetch \
    them with the `lucidos` CLI on your PATH:\n\
    - `lucidos knowhow list` — the full catalog (id + one-line description of every \
    available doc; there is more than just the app guides).\n\
    - `lucidos knowhow read <id>` — load one doc's full content.\n\
    Start with `lucidos knowhow read system-knowhow/building-an-app` (when an app is the \
    right answer, scaffolding defaults, common mistakes). Before writing app JS read \
    `system-knowhow/js-sdk` (the `lucidos.*` SDK surface is small but easy to misremember); \
    for file layout and where app data lives read `system-knowhow/best-practices`. Load the \
    relevant knowhow before writing app code rather than guessing.";

/// Codex-only teaching appended to every Codex-bound system prompt by
/// [`append_backend_rules`]. Two gaps it closes (see ADR 0004 follow-up +
/// `docs/plans/2026-06-12-codex-workspace-writes-and-user-questions.md`):
/// CC sessions get the lucidos-cli skill installed into the worktree and the
/// native `AskUserQuestion` tool; Codex gets neither, so without this section
/// a Codex session can't land files in the workspace's `data/` tree and
/// guesses instead of asking. Deliberately condensed — it costs tokens on
/// every fresh Codex session. The AGENTS.md alternative was rejected
/// (dirty-diff in external repos + shared-git-dir exclude leakage).
const CODEX_SESSION_RULES: &str = "\n\n\
    LUCIDOS CLI: The `lucidos` CLI is on your PATH. Your sandbox only permits writes inside \
    this worktree, but the CLI talks HTTP to the parent Lucidos engine (network is enabled), \
    so it works where direct writes are blocked. Use it whenever output belongs in the parent \
    workspace rather than in this worktree's source tree:\n\
    - `lucidos data write <relative> [--from <file>|-]` — write a file under the workspace's \
    `data/` tree (artifacts/, knowhow/, apps/, triggers/). Writing such files with your editor \
    tools or scripts puts them inside the worktree, where the engine cannot serve them and \
    links 404. `lucidos data path <relative> --mkdir` prints the resolved absolute path.\n\
    - `lucidos events emit <EventType> --summary \"...\" --payload '{...}'` — emit a domain \
    event (PascalCase past tense, e.g. `AnalysisCompleted`) to the workspace event store; \
    `lucidos events query [--type T] [--limit N]` reads prior events.\n\
    - `lucidos changes list` / `lucidos changes apply <id>` — list / apply a pending change. \
    Never hand-roll the HTTP call with curl — the CLI forwards the subprocess-origin headers \
    so the action is attributed to the agent, not the user.\n\
    - `lucidos spawn-thread --to <workspace> [--cc] --message ... --title ...` — spawn a new \
    Lucidos thread (always ask the user before spawning).\n\n\
    ASKING USERS: When you need the user's decision — a yes/no, picking between approaches, \
    choosing from a short list — call the `ask_user_question` tool (on the `lucidos` MCP \
    server) instead of guessing or asking in plain text. The Lucidos UI renders the options \
    as clickable buttons and the call blocks until the user answers, so you get a real answer \
    mid-turn. Arguments: `question` (required, the full question text), `options` (2-4 short \
    answer labels; omit for free-text), `multi_select` (allow picking several). One question \
    per call. An answer of `(canceled)` means the user dismissed the question — stop and wait \
    for their next instruction instead of re-asking. Do not guess when the tool can ask.";

/// Append backend-specific rules to a finished system prompt. Claude Code
/// prompts pass through unchanged (CC gets the lucidos-cli skill file + the
/// native `AskUserQuestion` tool instead); Codex prompts gain
/// [`CODEX_SESSION_RULES`]. Called from `resolve_run_worktree_context` with
/// the backend `run_direct_agent` already resolved — do NOT re-query
/// `thread_summaries` here.
pub(super) fn append_backend_rules(
    prompt: String,
    coding_agent: crate::runtime::CodingAgent,
) -> String {
    match coding_agent {
        crate::runtime::CodingAgent::ClaudeCode => prompt,
        crate::runtime::CodingAgent::Codex => format!("{prompt}{CODEX_SESSION_RULES}"),
    }
}

/// Build the system prompt for Lucidos-source coding-agent threads.
/// Used by both user-initiated Claude Code sessions and LLM-invoked
/// `claude_code` tool calls when editing the Lucidos source tree. Three
/// sibling builders exist for the other worktree flavors:
/// `external_repo_system_prompt`, `app_worktree_system_prompt`.
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
         {task_lifecycle}\n\n\
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
        task_lifecycle = TASK_LIFECYCLE_RULE,
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
         You have full git access — push and open PRs as needed; the user's git credentials and \
         CLI tools (gh, etc.) are available.\n\n\
         STAY ON THIS WORKTREE'S BRANCH: Do ALL of your committed work on `{branch_name}` — the \
         branch this worktree is checked out on. Lucidos tracks THIS branch for the thread's Diff \
         view and for resuming you, and the branch you push must be the same one. If the repo's \
         workflow wants a differently-named branch (e.g. a ticket branch like `UA-1234-...`), \
         RENAME this branch in place with `git branch -m {branch_name} <new-name>` and keep \
         working on it — do NOT `git checkout -b` a separate sibling branch, commit there, and \
         leave this worktree behind. Stranding later commits (e.g. a pre-PR cleanup pass) on a \
         branch this worktree is not on makes the Diff show stale, pre-cleanup work that no longer \
         matches your PR. Whatever branch you finish on MUST be the one this worktree is checked \
         out on — run `git branch --show-current` before you finish to confirm.\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {ask_user_question}\n\n\
         {task_lifecycle}\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}{permission_config}",
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
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
         {task_lifecycle}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{permission_config}",
        branch = branch_name,
        repo = repo_name,
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        process_safety = process_safety_rule(false),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for recovered Lucidos-source worktree sessions.
/// The LLM already has the original thread's message history, so this prompt
/// just explains the restart context and tells it to review and continue.
/// Three sibling recovery builders exist for the other worktree flavors:
/// `external_repo_recovery_system_prompt`, `app_worktree_recovery_system_prompt`.
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
         {task_lifecycle}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{permission_config}",
        preamble = workspace_preamble(workspace_name),
        branch = branch_name,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        apply_restart = APPLY_RESTART_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        process_safety = process_safety_rule(true),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for app coding-agent threads — sparse-checkout
/// worktrees of the user's workspace git narrowed to a single
/// `data/apps/<id>/` folder.
///
/// `app_manifest_json` is the parsed contents of the app's `manifest.json`,
/// serialized as a pretty JSON string. The agent reads this inline; other
/// app artifacts (intents, knowhow, scripts) are discovered via Read on
/// demand.
pub(super) fn app_worktree_system_prompt(
    branch_name: &str,
    workspace_name: &str,
    app_id: &str,
    app_manifest_json: &str,
) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         You are editing the `{app_id}` app in this workspace.\n\n\
         APP WORKTREE CONTEXT: You are running in an isolated git worktree of the user's \
         Lucidos *workspace* git (not the Lucidos source repo). The worktree is \
         sparse-checkout-narrowed to a single app folder on branch `{branch_name}`. \
         Your cwd is the app folder at `data/apps/{app_id}/`. The worktree root sits two \
         levels up, but only this app folder (plus top-level files like the workspace \
         `.gitignore`) is materialised. Other app folders, knowhow, triggers, artifacts — \
         all gitignored from your view via sparse-checkout.\n\n\
         APP MANIFEST:\n{app_manifest_json}\n\n\
         The rest of the app's structure — `index.html`, knowhow / intents / scripts \
         files, etc. — is on disk inside the app folder; use Read on demand.\n\n\
         {app_knowhow}\n\n\
         ISOLATION RULES: Your worktree is your entire world. ALL file edits MUST happen \
         inside the app folder under your cwd. Don't reach for absolute paths to other \
         workspace folders — the Apply review surface shows every changed file across the \
         worktree, so accidental writes are visible to the user, but you should narrow \
         your edits to this app folder by default. For workspace-wide data (knowhow, \
         triggers, artifacts, intents outside this app), use the `lucidos` CLI in \
         `run_bash` — that writes to live workspace data on `main`, not into your worktree.\n\n\
         You don't have `cargo`, `npx tsc`, `scripts/web-dev.sh`, or any Lucidos-source \
         build tooling here. Run the app's own test/lint commands if it ships any.\n\n\
         APPLY: When you finish, the user sees a pending *change* in the Apply panel. \
         Apply ff-merges your branch into the workspace git's `main`. **No engine \
         restart** ever happens (data-tree changes don't restart the engine). **No \
         `/harden`** runs (apps own their hardening; if this app ships its own \
         `.claude/commands/harden.md` use it on demand, otherwise rely on your own \
         bug-check pass). Apply emits a transient `AppUiRefreshRequested` if you touched \
         any iframe-bundled file (`index.html`, CSS, JS, `manifest.json`, static assets) \
         — open iframes of this app will reload to pick up your changes.\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check \
         for uncommitted changes. If you abandoned an approach and took a different one, \
         the old edits may still be in the working tree. Discard them with `git \
         checkout -- <file>`. Only intentional changes should remain.\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`.\n\n\
         NO PULL REQUESTS: This workspace's git is local — there is no remote and no PR \
         workflow. Never run `gh pr create`, never `git push` your branch, never tell the \
         user to \"open a PR\" or \"submit a PR\". The engine is the merge mechanism: when \
         the user clicks Apply, your branch lands on the workspace git's `main`.\n\n\
         {ask_user_question}\n\n\
         {task_lifecycle}\n\n\
         SESSION SUMMARY: Output a structured summary of what was implemented in this \
         session. List each change with a brief description. This is the last thing you \
         output before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or \
         stop, simply say goodbye and finish your response — the Lucidos engine manages \
         your lifecycle. Running `exit` in bash can crash the host application.{process_safety}{permission_config}",
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        app_knowhow = APP_KNOWHOW_RULE,
        process_safety = process_safety_rule(false),
        permission_config = PERMISSION_CONFIG_RULE,
    )
}

/// Build the system prompt for recovered orphaned app worktree sessions.
pub(super) fn app_worktree_recovery_system_prompt(
    branch_name: &str,
    workspace_name: &str,
    app_id: &str,
) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         You are editing the `{app_id}` app in this workspace.\n\n\
         RECOVERED SESSION: The Lucidos engine restarted while you were working on branch \
         `{branch_name}` in an isolated app worktree (sparse-checkout of the workspace git \
         narrowed to `data/apps/{app_id}/`). Your previous session was interrupted but \
         the worktree is intact.\n\n\
         Review the message history above to understand what you were working on, then:\n\
         1. Run `git log --oneline main..HEAD` to see what commits were made\n\
         2. Run `git diff` to check for uncommitted changes\n\
         3. If the work looks complete, clean up and finish\n\
         4. If incomplete, continue where you left off\n\n\
         When you finish, the user sees a pending *change* in the Apply panel. Apply \
         ff-merges your branch into the workspace git's `main`. No engine restart; no \
         `/harden` (apps own their hardening). Apply emits `AppUiRefreshRequested` if any \
         iframe-bundled file changed.\n\n\
         {app_knowhow}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check \
         for uncommitted changes. Discard unintentional changes with `git checkout -- <file>`.\n\n\
         COMMANDS: Never use /cpa. Just commit with `git add` + `git commit -m \"…\"`.\n\n\
         {ask_user_question}\n\n\
         {task_lifecycle}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{permission_config}",
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        app_knowhow = APP_KNOWHOW_RULE,
        process_safety = process_safety_rule(false),
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
     AFTER the commit succeeds, write ONE short final assistant message that summarizes \
     what you resolved — name each file and one sentence on the merge decision. \
     The user opens this recovery thread to see what happened; without a summary the \
     thread sits at Idle with no visible signal that the merge succeeded, and the user \
     has to git-log + diff to reconstruct the resolution themselves. Example: \
     \"Resolved 3 conflicts. crates/foo/src/lib.rs — kept main's signature, merged \
     your error-handling. crates/bar/src/mod.rs — combined both new tests. \
     packages/baz/index.ts — adopted main's import order around your new export.\"\n\n\
     If any conflict is ambiguous, ask the user before proceeding.\n\n\
     COMMANDS: Never use /cpa — it is for the main working tree only. \
     Just use `git add` + `git commit` directly.\n\n\
     CRITICAL: Never run `exit` as a bash command."
}

/// Build a merge prompt for Claude Code sessions.
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
    fn app_worktree_prompt_inlines_manifest_and_branch() {
        let manifest = r#"{"name":"Momentum","icon":"target"}"#;
        let prompt = app_worktree_system_prompt(
            "claude-code/app/momentum/20260527-100000-abc123",
            "personal",
            "momentum",
            manifest,
        );
        assert!(
            prompt.contains("`momentum`"),
            "must name the app id so CC knows which folder it owns",
        );
        assert!(
            prompt.contains("personal"),
            "must name the workspace so cross-workspace context is clear",
        );
        assert!(
            prompt.contains("claude-code/app/momentum/20260527-100000-abc123"),
            "must surface the branch name for the user-side Apply chip",
        );
        assert!(
            prompt.contains("Momentum"),
            "must inline the manifest so CC has the app's display name without an extra Read",
        );
        assert!(
            prompt.contains("AppUiRefreshRequested"),
            "must mention the iframe refresh signal so CC knows Apply is non-destructive",
        );
        assert!(
            prompt.contains("`/harden`"),
            "must explicitly opt out of /harden so CC doesn't try to run it",
        );
        // The sparse-checkout worktree can't see `system-knowhow/` on disk and
        // app sessions have no `load_knowhow` tool, so the prompt must point at
        // the `lucidos knowhow` CLI and name building-an-app as the entry doc.
        assert!(
            prompt.contains("lucidos knowhow read system-knowhow/building-an-app"),
            "must point app sessions at building-an-app knowhow via the CLI",
        );
        assert!(
            prompt.contains("lucidos knowhow list"),
            "must tell app sessions the full knowhow catalog is available via the CLI",
        );
        // App prompts must not advertise Lucidos-source scripts (cargo,
        // npx tsc, web-dev.sh, e2e.sh) — those don't resolve from the app
        // worktree's cwd. `/harden` and `/cpa` are intentionally NAMED in
        // the prompt (as opt-outs), so the Lucidos-only-token check is
        // narrower for app prompts than for external-repo ones.
        for token in &["./scripts/stop.sh", "./scripts/web-dev.sh", "./scripts/e2e"] {
            assert!(
                !prompt.contains(token),
                "app_worktree_system_prompt must not advertise `{token}` — it does not resolve in app worktrees",
            );
        }
    }

    #[test]
    fn app_worktree_recovery_prompt_inlines_branch_and_app() {
        let prompt = app_worktree_recovery_system_prompt(
            "claude-code/app/momentum/20260527-100000-abc123",
            "personal",
            "momentum",
        );
        assert!(prompt.contains("`momentum`"));
        assert!(prompt.contains("claude-code/app/momentum/20260527-100000-abc123"));
        assert!(prompt.contains("RECOVERED"));
        // A resumed app session writes app code too — it needs the same
        // knowhow pointer as a fresh app spawn.
        assert!(
            prompt.contains("lucidos knowhow read system-knowhow/building-an-app"),
            "recovery app prompt must also point at building-an-app knowhow via the CLI",
        );
        for token in &["./scripts/stop.sh", "./scripts/web-dev.sh", "./scripts/e2e"] {
            assert!(
                !prompt.contains(token),
                "app_worktree_recovery_system_prompt must not advertise `{token}`",
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
    fn external_repo_prompt_tells_cc_to_stay_on_the_worktree_branch() {
        // Regression guard: the prompt used to say "Create feature branches",
        // which let CC fork a sibling branch off the worktree's tracked branch,
        // commit a pre-PR cleanup pass there, push it, and leave the worktree
        // stranded on the pre-cleanup commit — so the Diff view (which follows
        // the worktree's branch) showed stale work that didn't match the PR.
        // The fix instructs CC to rename in place rather than fork.
        let prompt = external_repo_system_prompt("Acme", "UA-1879-fix", "origin/main");
        assert!(
            prompt.contains("git branch -m UA-1879-fix"),
            "external prompt must tell CC to RENAME its branch in place, not fork a sibling",
        );
        assert!(
            !prompt.contains("Create feature branches"),
            "external prompt must not invite CC to create sibling feature branches",
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
            assert!(
                prompt.contains("NEVER parallel-call"),
                "{label} must forbid parallel-calling `AskUserQuestion` alongside other \
                 tools (see ASK_USER_QUESTION_RULE)",
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

    /// Background-task lookup tools (`TaskOutput`, `TaskUpdate`, `TaskList`)
    /// return "Task not found" / "task already completed" once the engine has
    /// evicted a finished task's registry record. Without explicit guidance,
    /// CC was treating these as failures and retrying — nightly workspace-
    /// learning flagged the same shape on two consecutive nights (5/day on
    /// `TaskOutput`, then 3 `TaskUpdate` in a single thread within 1 minute).
    /// Pin the lifecycle note in every chat-style prompt so the inoculation
    /// can't be dropped by a future edit. The conflict-resolution prompt is
    /// intentionally excluded — it doesn't run bg tasks.
    #[test]
    fn chat_style_prompts_carry_task_lifecycle_note() {
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
                prompt.contains("TASK LIFECYCLE"),
                "{label} must carry the TASK LIFECYCLE rule so CC stops retrying \
                 \"Task not found\" errors on completed bg tasks",
            );
            // Assert the exact comma-joined trio phrase. A per-tool
            // `prompt.contains("TaskOutput")` would trivially pass because
            // ASK_USER_QUESTION_RULE also mentions `TaskOutput` ("no Bash,
            // no Read, no TaskOutput, no second AskUserQuestion") — so a
            // future edit could drop TASK_LIFECYCLE_RULE entirely and the
            // single-name check would still be satisfied. Pinning the
            // contiguous phrase guarantees the trio is named *together* and
            // can only come from TASK_LIFECYCLE_RULE.
            assert!(
                prompt.contains("`TaskOutput`, `TaskUpdate`, or `TaskList`"),
                "{label} must name all three lookup tools in the lifecycle \
                 rule's exact comma-joined form — partial coverage would let \
                 CC keep retrying the missing tool against stale ids",
            );
            // The expected-behavior framing is what stops the retry — without
            // it, CC reads the named tools and the "not found" string as
            // diagnosis instructions instead of as a benign signal.
            assert!(
                prompt.contains("expected"),
                "{label} must frame the error as expected behavior, not a bug",
            );
            assert!(
                prompt.contains("do NOT retry") || prompt.contains("do not retry"),
                "{label} must explicitly tell CC not to retry the call",
            );
        }
    }

    /// The Codex teaching section must be appended for Codex and ONLY for
    /// Codex: CC sessions already get the lucidos-cli skill file + the native
    /// `AskUserQuestion` tool, so duplicating the teaching there wastes
    /// context; a Codex session without it can't land workspace `data/` files
    /// (the sandbox blocks direct writes) and guesses instead of asking.
    #[test]
    fn codex_prompts_carry_cli_and_question_teaching() {
        let base = worktree_system_prompt("feature/x", "dev");
        let codex = append_backend_rules(base.clone(), crate::runtime::CodingAgent::Codex);
        for needle in [
            "lucidos data write",
            "lucidos events emit",
            "lucidos changes apply",
            "lucidos spawn-thread",
            "ask_user_question",
            "(canceled)",
        ] {
            assert!(
                codex.contains(needle),
                "Codex prompt must teach `{needle}` — the sandboxed session has no other \
                 path to workspace writes / user questions",
            );
        }
        assert!(
            codex.starts_with(&base),
            "backend rules must append, not replace, the worktree prompt",
        );

        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        assert_eq!(
            cc, base,
            "Claude Code prompts must pass through unchanged — CC gets the lucidos-cli \
             skill + native AskUserQuestion instead",
        );
        assert!(
            !cc.contains("lucidos data write"),
            "the CC base prompt must not duplicate the CLI teaching",
        );
    }

    /// Conflict-resolution sessions run unattended in a temp worktree — the
    /// user never sees a back-and-forth with them. When CC finishes (commits
    /// the merge) and the engine ff-merges to main, the thread sits in
    /// "Idle" with whatever CC happened to say last. If CC was terse ("done."
    /// or no text at all) the user opens the thread and sees no closure —
    /// the original bug the user complained about: "It just stopped. Its
    /// output didnt say it was resolved."
    ///
    /// Pin that the prompt requires a one-sentence summary as CC's final
    /// assistant message so the recovery thread always carries a visible
    /// statement of what was resolved.
    #[test]
    fn conflict_resolution_prompt_requires_user_facing_summary() {
        let prompt = conflict_resolution_system_prompt();
        let prompt_lower = prompt.to_lowercase();
        // Concept words — multiple acceptable phrasings (summary / summarize /
        // explain) so future rewrites can reword without tripping the test,
        // but at least one of these MUST appear to ensure the closure
        // message stays an explicit instruction, not optional.
        assert!(
            prompt_lower.contains("summar") || prompt_lower.contains("explain what"),
            "conflict_resolution_system_prompt must instruct CC to summarize what it \
             resolved as its final assistant message — otherwise terse CC turns \
             ('done.', empty text) leave the user with no closure: the recovery \
             thread sits at Idle with no visible signal that the merge succeeded"
        );
        // The summary must be the LAST step, after the commit. If CC sends
        // a summary BEFORE the commit, the engine's ff-merge hasn't happened
        // yet and the message would be misleading.
        let commit_pos = prompt
            .find("git commit")
            .expect("prompt must mention the merge commit step");
        let summary_pos = prompt_lower
            .find("summar")
            .or_else(|| prompt_lower.find("explain what"))
            .expect("checked above");
        assert!(
            summary_pos > commit_pos,
            "the summary instruction must come AFTER the commit step in the prompt — \
             a pre-commit summary would mislead the user about whether the merge \
             actually succeeded"
        );
    }
}
