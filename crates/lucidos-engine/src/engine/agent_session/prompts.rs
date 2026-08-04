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
         For e2e, run `./scripts/e2e.sh` directly — it builds and boots its own \
         session-scoped engine and cleans up after itself. Do NOT pre-start with \
         `./scripts/web-dev.sh`: that launches the machine-global gateway, which is \
         refused from a worktree (ADR 0021)."
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

/// Commit-cadence guidance shared across coding-agent prompts. The engine
/// updates the review surface from commits, so agents should checkpoint
/// coherent finished slices while they work without creating noisy
/// commit-per-edit history.
const COMMIT_CADENCE_RULE: &str = "COMMIT CADENCE: Commit completed, coherent slices of work \
    as you go, not only at the end. Use `git status` and `git diff` to review what will be \
    included before each commit. Do not commit after every tiny edit; do commit after each \
    self-contained fix, feature slice, cleanup checkpoint, or other reviewable unit so the \
    Diff view and recovery state stay current. Avoid committing known-broken work unless you \
    are explicitly checkpointing an intermediate state and the commit message says so.";

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

/// Implementation-planning rule shared by the two Lucidos-source prompts
/// (`worktree_system_prompt`, `recovery_system_prompt`). Lives in the shared
/// base — NOT in a backend section — so it reaches BOTH Claude Code (via
/// `--append-system-prompt`) and Codex (via `developerInstructions`). It is the
/// soft, prospective half of enforcement; the hard halves are the Claude-Code
/// `cc-plan-gate` PreToolUse hook (blocks the first source edit until a
/// gate-satisfying — recorded AND approved — marker exists) and the Apply floor
/// (refuses a missing-or-unapproved Lucidos-source change). Codex has no
/// PreToolUse hook, so for Codex this rule + the Apply floor are the whole
/// enforcement. Keep in sync with the `implementation-plan` skill
/// (`.claude/skills/implementation-plan/SKILL.md`) and `lucidos planned`.
const IMPLEMENTATION_PLAN_RULE: &str = "IMPLEMENTATION PLAN: Before your FIRST code edit, decide \
    whether this is complex work — ADR- or design-thread-backed, cross-layer, any routing / \
    topology / storage / security / migration / process change, or anything beyond a local bug \
    fix. If it is, produce an implementation plan FIRST: run the `implementation-plan` skill \
    (`.claude/skills/implementation-plan/SKILL.md`) — it turns the prompt, any grill/design \
    thread, ADRs, and code reconnaissance into `docs/plans/<date>-<slug>.md` and records a \
    PROPOSED plan marker via `lucidos planned mark --plan <path>`. A proposed plan does NOT \
    unblock editing: present the plan to the user, then ASK FOR APPROVAL WITH THE QUESTION TOOL \
    named in the ASKING USERS section, offering `Approve` and `Request changes`. That pair is a \
    FLOOR: `Approve` first, `Request changes` second ONLY when the plan offers no real fork. If \
    it offers one (a narrower scope, one layer instead of two), that fork takes the second slot \
    and `Request changes` is dropped, never carried alongside it as a third. The approval itself \
    is a DECISION question, not a post-work confirmation: the edit gate is closed until they \
    answer, so approval asked in plain prose just leaves the thread sitting idle. Once the user \
    approves, run `lucidos planned approve` to flip the marker to gate-satisfying. Only then do \
    source edits and Apply unblock. Picking a fork is an approval too: revise the plan file to \
    that variant, re-commit, then run `lucidos planned approve`. If the user requests changes \
    instead, revise the plan file, re-commit, and present it again, asking the same way (the \
    marker stays proposed until approved). If this is genuinely a local fix, acknowledge that \
    instead with \
    `lucidos planned mark --simple \"<one-line reason>\"` (no \
    approval needed). A gate-satisfying marker MUST exist before the change can be applied: Claude \
    Code blocks your first source edit until one is set and approved, and Apply refuses a \
    marker-less or unapproved change. Writing the plan file itself under `docs/plans/` is never \
    blocked. Keep the plan's load-bearing invariants in view while you edit; do not defer their \
    first appearance to `/harden`.";

/// Restart-is-not-rejection note shared by the three recovery system prompts
/// (`recovery_system_prompt`, `external_repo_recovery_system_prompt`,
/// `app_worktree_recovery_system_prompt`). A session resumed after an engine
/// restart replays its own transcript, which may contain a permission denial
/// ("User denied"), an interrupted/incomplete tool call, or a synthetic
/// "[Request interrupted…]" result — all artifacts of the restart, NOT user
/// decisions. Without this, the resumed agent reads them as the user rejecting
/// its approach and changes course. Kept repo-generic (no Lucidos-only tokens)
/// so it is safe in the external-repo recovery prompt. The companion half is the
/// neutral `RESTART_INTERRUPT_REASON` returned on the permission teardown path
/// (`engine::cc_permission`).
const RESTART_NOT_REJECTION_RULE: &str = "RESTART CONTEXT — NOT A REJECTION: This session was \
    resumed after the engine restarted mid-work. If your recent history shows a permission denial \
    (e.g. \"User denied\"), a tool call that was interrupted or never completed, or a synthetic \
    \"[Request interrupted]\" result, that was caused by the restart — NOT by the user rejecting \
    your work, your plan, or your approach. Do not abandon or rework your approach on account of \
    those signals. Re-confirm where you left off (the git log/diff steps above) and continue the \
    same plan, unless the user has since told you otherwise in a new message.";

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

/// Tell the coding agent that background processes do NOT outlive the turn that
/// started them. A coding-agent session is a per-turn subprocess: when the turn
/// goes idle the engine tears down the whole process group (the agent + every
/// child it spawned — see `lifecycle::terminate_decision` +
/// `runtime::spawn_env::graceful_kill_child_process_group`), and nothing
/// re-invokes the agent when a backgrounded job would later finish (that wake
/// path exists only for the *chat* agent's tracked `run_bash_background` tool,
/// which coding agents don't have). Left to its own devices the agent trusts its
/// Bash tool's native "runs across turns, re-invokes you on exit" contract —
/// true for the standalone CLI, false here — so it kicks off a long build in the
/// background, ends the turn, and the build is killed; the next `--resume` turn
/// starts it over. This bit us in a real DMG-build thread (three restarts, no
/// artifact). The fix is guidance: foreground long commands, or wait them out
/// within the same turn.
///
/// The second half of the rule is about the COST of that wait. "Wait inside the
/// turn" on its own reads as "tick until done", and the agent's cheapest-looking
/// tick is the tool default — 120000 ms on CC's Bash tool — so a 40-minute
/// release build turned into ~20 round-trips, each dragging a log tail into
/// context. Both waits can block instead: a foreground command accepts an
/// explicit `timeout` up to 600000 ms, and `TaskOutput` takes `block: true` with
/// the same ceiling and returns the moment the task exits. Naming those two
/// ceilings is what turns ~20 polls into ~4. Spell the numbers the same way here
/// as in the prompt string and the test needles (`600000`, not `600 000`) so one
/// grep finds every site when a ceiling moves.
///
/// The foreground half carries a trap worth naming in the prompt: overrunning
/// the tool timeout KILLS the command, so "max out the timeout" is only safe
/// when the run fits — an uncertain estimate should go to the background path,
/// which has no such cliff. Applies to chat-style prompts only (grouped with
/// `TASK_LIFECYCLE_RULE`); merge-conflict sessions don't run builds.
const BACKGROUND_PROCESS_RULE: &str = "BACKGROUND PROCESSES DON'T SURVIVE A TURN: When your turn \
    ends (you go idle), the Lucidos engine terminates your whole process group — you and every \
    process you spawned. A command started with `run_in_background` (or `&` / `nohup` / any \
    detached job) is therefore KILLED the instant you end the turn, and nothing re-invokes you \
    when it would have finished; the next turn just starts it over from scratch. So NEVER kick \
    off a long-running command in the background and then end your turn expecting to be woken \
    with its result — you won't be. Instead: run the command in the FOREGROUND (that keeps your \
    turn open while it runs), or, for work too long for a single foreground command's tool \
    timeout, start it in the background and wait it out WITHIN THE SAME TURN. Only finish once \
    you actually have the command's result. WAIT IN AS FEW CALLS AS YOU CAN — having to wait is \
    not a reason to poll on a short tick. Foreground: set the timeout EXPLICITLY to its maximum \
    (Claude Code's Bash tool takes `timeout: 600000`, i.e. 10 minutes; its 120000 ms DEFAULT is \
    what silently cuts a long build off at 2 minutes) so one blocking call covers the whole run \
    — but OVERRUNNING that ceiling kills the command and throws the work away, so send anything \
    that might exceed 10 minutes to the background instead of gambling on the estimate. \
    Background: call `TaskOutput` with `block: true` and `timeout: 600000` — it returns the \
    instant the task exits, or after 10 minutes; re-issue it until the task is done, with no \
    cliff if you guessed the duration wrong. A 40-minute build costs ~4 blocking calls that way, \
    versus ~20 polls at the 2-minute default, each one a full round-trip that floods your \
    context with log tails. Tick on a short interval only when you deliberately want live \
    progress or an early abort — never as the default way to wait.";

/// Encourage the coding agent to ask via the structured `AskUserQuestion` tool
/// — which the Lucidos UI renders as clickable buttons — for the DECISIONS it
/// needs to proceed, while forbidding post-work "does this look good?"
/// confirmations. The forbidding half is load-bearing: a held-open question
/// parks the thread in `waiting_for_user_answer`, which stalls hand-off (in an
/// Apply-based worktree it blocks the Apply button — see
/// [`APPLY_CONFIRMATION_NOTE`]), and for a visual/behavioral change the user
/// can't even judge the result until it's landed — the two lock each other out
/// (the "cant apply when you ask question" dark-mode-contrast thread). The prior
/// wording actively told the agent to turn end-of-turn confirmations into button
/// questions, which caused exactly that deadlock.
///
/// The forbidding half then over-reached, so the rule carries an explicit
/// carve-out for **plan approval**. A plan written and committed by the
/// `implementation-plan` skill reads to the model as work it has ALREADY done,
/// so "never CONFIRM finished work" swallowed the one approval the plan marker
/// depends on: the agent presented the plan in prose and the thread sat idle
/// until the user typed "approve" by hand (2026-08-02). It is the opposite
/// case in every way that matters. The plan is a proposal about work NOT done,
/// the `cc-plan-gate` hook blocks source edits until the answer arrives, and
/// nothing is appliable yet. Keep the carve-out in sync with
/// [`IMPLEMENTATION_PLAN_RULE`], `cc_plan_gate::build_awaiting_approval_json`
/// and the `implementation-plan` skill, which all describe the same option
/// FLOOR: `Approve` first, `Request changes` second ONLY when the plan offers
/// no real fork, never a third slot beside one. Those surfaces stated the pair
/// unconditionally until 2026-08-04, which produced a live three-option card
/// (`Approve` / `Frontend only` / `Request changes`) whose last button meant
/// only "I will type what I want changed". That is the exact shape the NEVER
/// AUTHOR AN "OTHER" OPTION paragraph below bans, and it is redundant with the
/// free-text escape the card already names.
///
/// The general lesson from that same incident is the "WHY THE TOOL AND NOT
/// PROSE" paragraph, and it is what makes the two halves cohere instead of
/// reading as a contradiction. Both describe ONE mechanism: a tool call parks
/// the thread in `WaitingForUserAnswer`, which is the only input to
/// `thread_lifecycle::is_attention_needing`. That predicate lights the
/// needs-attention badge, keeps the thread in `DisplaySection::Current` even
/// once archived, bubbles up the ancestor chain via
/// `attention_descendant_count`, and is a fire condition for the "When agent
/// needs me" trigger, so it is also what can notify the
/// user. Parking is therefore the COST when the work is finished (the reason
/// for the forbidding half) and the POINT when the agent is blocked. A prose
/// question is not the gentle middle option the model reads it as: the turn
/// ends, nothing is marked, and the thread is indistinguishable from a
/// completed one, so the user never learns anyone is waiting.
///
/// This shared rule is kept ENVIRONMENT-GENERIC (no "Apply" / "the change be
/// proposed" / "Diff") because it is interpolated into external-repo prompts
/// too, where there is no Apply — those sessions push and open PRs. The
/// Apply-specific sharpening lives in [`APPLY_CONFIRMATION_NOTE`], added only by
/// the Apply-based prompt builders. Applies to chat-style prompts only;
/// hardening and merge-conflict sessions don't dialogue with the user. The
/// regression test `chat_style_prompts_nudge_use_of_ask_user_question` pins both
/// halves.
///
/// The "NEVER AUTHOR AN \"OTHER\" OPTION" paragraph exists to CONTRADICT Claude
/// Code's own built-in tool description, which promises "there should be no
/// 'Other' option, that will be provided automatically". CC's TUI provides one;
/// Lucidos does not. The card renders exactly the options passed, and
/// `agent_question::answer_kind_to_hook_value` resolves a `Selected` answer to
/// the option's LABEL (via `lookup_option_label`), so an "Other, I'll type it"
/// button hands that literal phrase back as the user's decision. The two real escapes (typing in the prompt
/// textarea, which routes to the pending question as `FreeText`, and Cancel)
/// are on every card already and are named to the user by the prompt
/// textarea's own placeholder while a question is pending
/// (`PLACEHOLDER_ANSWERING`). Mirrored into [`CODEX_ASK_USER_QUESTION_RULE`] (which
/// REPLACES this whole constant for Codex, so the ban must be restated there,
/// not appended here) and into the chat-side rule + `ask_user_question` tool
/// description in `llm::tools::misc`; change them together.
const ASK_USER_QUESTION_RULE: &str =
    "ASKING USERS: Use the `AskUserQuestion` tool when you need a DECISION from the \
     user to move forward — which of two approaches to take, an ambiguous requirement, a \
     judgment call you can't make yourself — and ask it BEFORE or WHILE you do the work, \
     when you actually need the answer to proceed. The Lucidos UI renders its options as \
     clickable buttons; options listed only in your message text force the user to type \
     their reply instead of clicking. ALWAYS provide the `question` field — the full \
     question text shown on the card; the optional `header` chip-label is never a \
     substitute, so don't put the question only in `header` (or only in your prose) and \
     leave `question` empty. The engine rejects a call whose `question` is missing and \
     makes you re-ask. Use `AskUserQuestion` for any such decision with 2-4 discrete \
     answers, including the binary yes/no case. Mid-stream decision questions (\"border or \
     bg?\", \"is this the right direction before I continue?\") are fine, and nothing is \
     blocked because there's no finished change yet. ASKING THE USER TO APPROVE A PLAN OR AN \
     APPROACH BEFORE YOU IMPLEMENT IT IS ALWAYS SUCH A DECISION: the plan is a proposal about \
     work you have NOT done, you cannot proceed without the answer, and there is nothing to hand \
     off yet. Route it through this tool with `Approve` and `Request changes` as the options. \
     That pair is a FLOOR, not a fixed shape. The tool requires at least two options, so a lone \
     `Approve` button is not expressible, and `Request changes` fills the second slot when the \
     plan offers no real fork. When it DOES offer one (a narrower scope, one layer \
     instead of two, a different approach), make that fork the second option and DROP `Request \
     changes`. The fork already satisfies the two-option minimum and carries a decision you can \
     act on, while a third `Request changes` beside it means only \"I will type what I want \
     changed\", which is the escape every card already has. `Approve` stays first either way. \
     Picking a fork is still an approval: it approves that variant of the plan, and is not a \
     rejection.\n\n\
     NEVER AUTHOR AN \"OTHER\" OPTION: do not add an option meaning \"Other\", \"Something \
     else\", \"Let me type it\" or \"I'll write my own answer\". Your tool description says an \
     \"Other\" option is provided automatically. In Lucidos it is NOT: the card renders exactly \
     the options you pass, every option is a label, and tapping one hands you that label back as \
     the user's answer, so an \"Other, I'll type it\" button arrives as their decision and leaves \
     you re-asking. Both escapes are on every card without you spending an option slot on them. \
     The user can type any reply in the prompt textarea and it arrives as their answer to this \
     question, and Cancel dismisses the question so they can steer you somewhere else. Options \
     are for the pre-baked choices only. An option that carries a decision you can act on is a \
     different thing and still welcome (\"None of these\", \"Neither, ask me later\", \"Cancel \
     the deploy\"); what is banned is an option whose only meaning is \"I will type it \
     instead\".\n\n\
     WHY THE TOOL AND NOT PROSE: the tool call is the ONLY thing that tells Lucidos you are \
     waiting. It parks the thread in the waiting-for-answer state, which is what lights the \
     needs-attention badge, keeps the thread in the live working set, and can push a \
     notification to the user's phone. A question you type into your final message instead does \
     none of that: the turn ends, the thread reads as FINISHED, and the user gets no signal \
     that you are stuck. So a prose question is not a lighter-touch version of the tool. When \
     you actually need an answer, it is silence. This is the same mechanism as the paragraph \
     below, seen from the other side: parking the thread is the COST when your work is done and \
     the user should be free to take it forward, and it is exactly the POINT when you cannot \
     proceed without them. So the two paragraphs never disagree about a given question, because \
     the resolutions differ. Blocked on the user: ask with the tool. Work finished: do not ask \
     AT ALL, and do not promote a trailing \"does this look good?\" into a tool call either. \
     Just hand it off. What is always wrong is the third thing: ending a turn with an \
     unanswered question sitting in your prose.\n\n\
     DO NOT ask a confirmation question about work you've ALREADY done or are wrapping up — \
     \"does this look good?\", \"does this look complete?\", \"did I miss anything?\", \
     \"want me to tweak the color?\". A held-open question parks the thread in the \
     waiting-for-answer state, which stalls hand-off — the user can't take your finished \
     work forward while a question is open. And for a visual or behavioral change the user \
     often cannot even judge the result until it's landed and running, so \"does this look \
     good?\" is unanswerable at that point. When the work is done, DON'T ask whether it's \
     good: finish and hand it off, and let the user review the result. If it needs tuning, \
     they'll tell you in a new message. Ask to DECIDE, never to CONFIRM finished work. Approving \
     a plan you have not implemented yet is a DECISION and is never covered by this paragraph, \
     however finished the plan document itself feels.\n\n\
     NEVER parallel-call `AskUserQuestion` alongside other tools — if you're asking a \
     question, stop the assistant message after the `AskUserQuestion` tool_use and do not \
     include any sibling tool_uses (no Bash, no Read, no TaskOutput, no second \
     AskUserQuestion). Lucidos's PreToolUse hook blocks `AskUserQuestion` for up to 24h, \
     but any sibling tool_uses in the same message dispatch in parallel and emit progression \
     events while the question is still on-screen — at which point the user's typed comment \
     can no longer be safely routed as a free-text answer, and your own parallel work has \
     wasted tokens on an unconfirmed direction. Wait for the answer, THEN continue.";

/// Apply-specific sharpening of the "never CONFIRM finished work" half of
/// [`ASK_USER_QUESTION_RULE`], added ONLY by the Apply-based prompt builders
/// (Lucidos-source worktree + recovery, app worktree + recovery). It names the
/// concrete mechanism — a held-open question parks the thread in
/// `waiting_for_user_answer`, which blocks the Apply button
/// (`is_blocking` / `available_thread_actions` in `thread_lifecycle.rs`). It is
/// deliberately NOT in the shared rule, because external-repo prompts have no
/// Apply (they push and open PRs) and the Apply-blocking rationale would
/// mislead them into stopping without pushing. Reaches both backends: it is a
/// separate placeholder in the builders, so `append_backend_rules`' Codex swap
/// of `ASK_USER_QUESTION_RULE` leaves it intact.
const APPLY_CONFIRMATION_NOTE: &str = "APPLYING YOUR WORK: Concretely, a question you leave open \
     parks this thread in the waiting-for-answer state, which BLOCKS the Apply button — the user \
     cannot Apply your change while a question is open, and for a visual change they can't judge \
     it until they Apply. So when your change is ready, finish the turn and let the change be \
     proposed, so the user can review the Diff, click Apply, and see it live — never gate a \
     finished change behind a \"does this look good?\" question.";

/// Permission allowlist rule — Claude Code ONLY. Lucidos passes
/// `--allowedTools` when spawning CC, which overrides settings.json permission
/// rules, so tool allowlist edits MUST go in `~/.lucidos/cc-allowed-tools`.
/// None of this applies to Codex (it uses its own sandbox + approval-policy
/// model — approval cards raised by the app-server's `requestApproval`, not
/// `--allowedTools`), so [`append_backend_rules`] appends this only in the
/// `ClaudeCode` arm — mirroring how [`CODEX_CLI_RULE`] is Codex-only.
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

/// Codex-only CLI teaching appended to every Codex-bound system prompt by
/// [`append_backend_rules`]. CC sessions get the lucidos-cli skill installed
/// into the worktree; Codex gets neither that skill nor a project AGENTS.md,
/// so without this section it can't land files in the workspace's `data/`
/// tree. Deliberately condensed — it costs tokens on every fresh Codex
/// session. The AGENTS.md alternative was rejected (dirty-diff in external
/// repos + shared-git-dir exclude leakage).
const CODEX_CLI_RULE: &str = "\n\n\
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
    - `lucidos spawn-thread --to <workspace> [--cc|--codex|--coding-agent <backend>] --message ... \
    --title ...` — spawn a new Lucidos thread. Use `--codex` when the user asks for Codex \
    (always ask the user before spawning).";

/// Codex-only slash-command mapping appended by [`append_backend_rules`]
/// alongside [`CODEX_CLI_RULE`]. Lucidos prompts name Claude Code slash
/// commands — the shared [`HARDENING_RULE`] ("run `/harden`"), the
/// merge-conflict prompt's harden step, and the engine's auto-harden
/// follow-up (`AUTO_HARDEN_MESSAGE`, "Run /harden now.") — but Codex has no
/// slash-command runtime, so without this mapping it has to guess what
/// `/harden` means. It guessed badly in practice: 17% of Codex changes hit
/// Apply with no harden marker vs 0.6% for CC (dev workspace, 2026-06/07),
/// each one paying the synchronous Apply-time hardening wait. Appended (not
/// a replace) so it defines the mapping once for every mention in any prompt
/// flavor, including the hardening-session override and merge prompts.
const CODEX_SLASH_COMMANDS_RULE: &str = "\n\n\
    SLASH COMMANDS: Prompts here may tell you to run a slash command such as `/harden`. That \
    is Claude Code skill syntax; you have no slash-command runtime — each one is a repo-owned \
    playbook file you execute by reading it and following its steps: `/harden` = \
    `.claude/commands/harden.md`, `/code-review` = `.claude/skills/code-review/SKILL.md` (the \
    general shape: `.claude/commands/<name>.md` or `.claude/skills/<name>/SKILL.md`). Running \
    `/harden` to completion is what records the hardened marker (the playbook uses the \
    `lucidos hardened` CLI; check state with `lucidos hardened query`) — never claim hardening \
    is done while that marker is missing. Skip a playbook step \
    only when the playbook itself says it does not apply to a Codex-backed run.";

/// Codex replacement for [`ASK_USER_QUESTION_RULE`]. Codex sessions do not
/// have Claude Code's native `AskUserQuestion` tool; their clickable-question
/// path is the Lucidos MCP server's `ask_user_question` tool. This replaces
/// the shared Claude-style rule inside [`append_backend_rules`] instead of
/// appending after it, so Codex never sees conflicting instructions.
const CODEX_ASK_USER_QUESTION_RULE: &str = "\
    ASKING USERS: When you need the user's decision — a yes/no, picking between approaches, \
    choosing from a short list — call the `ask_user_question` tool (on the `lucidos` MCP \
    server) instead of guessing or asking in plain text. The Lucidos UI renders the options \
    as clickable buttons and the call blocks until the user answers, so you get a real answer \
    mid-turn. Arguments: `question` (required, the full question text), `options` (2-4 short \
    answer labels; omit for free-text), `multi_select` (allow picking several). One question \
    per call. An answer of `(canceled)` means the user dismissed the question — stop and wait \
    for their next instruction instead of re-asking. Do not guess when the tool can ask. \
    Ask to DECIDE, never to CONFIRM finished work: do NOT ask a \"does this look good / \
    complete?\" question about a change you've already made. A held-open question just parks \
    the thread and stalls hand-off, and the user can't judge a visual result until it's \
    landed and running — finish and hand it off instead so they can review it. \
    Approving a plan BEFORE you implement it is the opposite case: a DECISION you cannot \
    proceed without, about work you have NOT done. Always ask for plan approval through this \
    tool, with `Approve` and `Request changes` as the options, never in plain prose. That pair \
    is a FLOOR: `Approve` comes first, and `Request changes` fills the second slot only when the \
    plan offers no real fork. When it offers one (a narrower scope, one layer instead of \
    two), make that fork the second option and drop `Request changes` rather than carrying it as \
    a third, where it would mean only \"I will type what I want changed\". Picking a fork is \
    still an approval: it approves that variant of the plan, and is not a rejection. \
    NEVER AUTHOR AN \"OTHER\" OPTION: no option meaning \"Other\", \"Something else\" or \
    \"Let me type it\". Every option is a label, and tapping one hands you that label back as \
    the user's answer, so such a button arrives as their decision and leaves you re-asking. \
    Both escapes are on every card without you spending an option slot: the user can type any \
    reply in the prompt textarea and it arrives as their answer to this question, and Cancel \
    dismisses the question so they can steer you elsewhere. Options are for the pre-baked \
    choices only. An option carrying a decision you can act on is different and still welcome \
    (\"None of these\", \"Neither, ask me later\"); what is banned is one whose only meaning is \
    \"I will type it instead\". \
    WHY THE TOOL AND NOT PROSE: the tool call is the ONLY thing that tells Lucidos you are \
    waiting. It parks the thread in the waiting-for-answer state, which lights the \
    needs-attention badge, keeps the thread in the live working set, and can notify the user. A \
    question typed into your final message instead ends the turn, so the thread reads as \
    FINISHED and the user gets no signal that you are stuck. Blocked on the user: ask with this \
    tool. Work finished: don't ask at all, just hand it off. What is always wrong is ending a \
    turn with an unanswered question sitting in your prose. \
    NEVER parallel-call `ask_user_question` alongside other tools — if you're asking a \
    question, stop the assistant message after the `ask_user_question` tool call and do not \
    include any sibling tool calls.";

/// Backend-INDEPENDENT teaching appended to every coding-agent prompt by
/// [`append_backend_rules`] — the shared chokepoint that rides every flavor
/// (normal, recovery, conflict, override) for both backends. Both Claude Code
/// and Codex run their reasoning in a channel the Lucidos UI does not render:
/// CC's extended-thinking blocks come back `display: "omitted"` (signature only,
/// no `thinking_delta`) by default for the current models and stay empty even
/// when summarized display is requested in headless `stream-json` mode (an
/// upstream CC limitation — see `runtime/claude_code_parse.rs` and the
/// `cc-reasoning-dormant` investigation in `docs/temporary-measures.md`), and
/// Codex streams only a lossy reasoning *summary* (`model_reasoning_summary`
/// — see `CODEX_REASONING_SUMMARY` in `runtime/codex.rs`), never the full
/// reasoning. So anything the model parks in its
/// reasoning is invisible (CC) or unreliably summarized (Codex) for the
/// user. Without this rule the agent drafts
/// user-facing content there and then references it as if shown — the real
/// "Caption copy: do the six lines above work?" card whose six lines never
/// appeared. We cannot extract the reasoning text (it is a summary at best, and
/// unavailable through the CC CLI we drive); the fix is guidance — tell the
/// agent its reasoning is not shown so it puts must-see content in a visible
/// message.
const REASONING_NOT_VISIBLE_RULE: &str = "\n\n\
    YOUR REASONING IS NOT SHOWN TO THE USER: In this UI your extended thinking / reasoning is \
    NOT displayed — the user sees only your visible assistant messages and your tool calls, never \
    your private reasoning. So any content the user must see or act on — draft copy you want \
    approved, the options behind a question, a snippet you want reviewed, a summary of what you \
    found — MUST go in a visible assistant message (or a structured tool field the UI renders, \
    such as a question tool's `question` / `options`), NEVER only in your reasoning. Do not \
    reference content as if the user can see it (\"the six lines above\", \"as shown in my \
    analysis\") unless you actually put it in a visible message this turn. When in doubt, write \
    it in the message.";

/// Backend-INDEPENDENT teaching appended to every coding-agent prompt by
/// [`append_backend_rules`], the same chokepoint [`REASONING_NOT_VISIBLE_RULE`]
/// rides.
///
/// A coding agent swims in identifiers: commit shas from `git log`, the change
/// id and short sha in the turn-gap note (`turn_gap::change_label` falls back
/// to `change abc12345` when a change has no description), its own branch name,
/// background task ids. None of them name anything the user can see. The
/// Lucidos UI labels a change by its thread title and its Diff, never by id, so
/// an assistant message built around one is unreadable: the user cannot tell
/// which change is meant, let alone decide about it.
///
/// Two carve-outs keep the rule presentation-only, and both are load-bearing.
/// Ids stay legal in git commands and tool arguments, or the agent stops using
/// them where they are required. And a **markdown link target** is not prose:
/// `lucidos spawn-thread` prints `[title](thread:<ws>/<uuid>)` for the agent to
/// paste (see `system-knowhow/lucidos-cli.md`), and that link is the user's
/// only way to open the thread it just started.
///
/// Mirrored for the chat agent by
/// `chat::process::system_prompt::NAMES_NOT_IDS_RULE` (same rule, tuned to
/// changes and the `changes` tool). Change both together.
const NAMES_NOT_IDS_RULE: &str = "\n\n\
    NAME THINGS THE WAY THE USER SEES THEM, NEVER A RAW ID OR SHA: Identifiers belong in \
    commands, not in prose. A commit sha, change id, thread id, task id, branch name, or any \
    other uuid/hex string is meaningless to the user: no screen in Lucidos is labelled with it, \
    so they cannot look it up and cannot act on it. NEVER put one in an assistant message, in a \
    question, or in an option label. Refer to the work by what it is: a commit by its subject \
    line, a change by what it does and which files it touches, a thread by its title, a file by \
    its path. Your session summary lists commit subjects and file paths, never shas. Ids stay \
    where they belong: git commands, tool arguments, and CLI calls still take them, and the \
    Diff view shows the user the real thing. A markdown link TARGET is not prose either, so \
    keep pasting `[title](thread:<ws>/<uuid>)` exactly as `lucidos spawn-thread` prints it: \
    the user reads the label and taps it, and without the link they cannot open the thread you \
    started. The only other exception is a raw value the user asked for, or one they have to \
    paste somewhere.";

/// Append backend-specific rules — plus the backend-independent
/// [`REASONING_NOT_VISIBLE_RULE`], which rides every prompt here — to a finished
/// system prompt — the single
/// point where the two coding-agent backends diverge. Claude Code prompts gain
/// [`PERMISSION_CONFIG_RULE`] (the `--allowedTools` / `~/.lucidos/cc-allowed-tools`
/// mechanics are CC-only); Codex prompts replace [`ASK_USER_QUESTION_RULE`]
/// with [`CODEX_ASK_USER_QUESTION_RULE`] and append [`CODEX_CLI_RULE`]. Each
/// backend gets ONLY its own section: the CC permission-config rule would be
/// misleading noise on a Codex session, which surfaces permissions through its
/// sandbox + approval-policy model (approval cards raised by the app-server),
/// not `--allowedTools`.
/// Called from `resolve_run_worktree_context` with the backend
/// `run_direct_agent` already resolved — do NOT re-query `thread_summaries` here.
pub(super) fn append_backend_rules(
    prompt: String,
    coding_agent: crate::runtime::CodingAgent,
) -> String {
    // Backend-INDEPENDENT: both backends hide the model's reasoning from the UI
    // and both talk to the user about work they track by sha, so every prompt
    // flavor gets these two rules here (the shared chokepoint) before the
    // backend-specific teaching below.
    let prompt = format!("{prompt}{REASONING_NOT_VISIBLE_RULE}{NAMES_NOT_IDS_RULE}");
    match coding_agent {
        crate::runtime::CodingAgent::ClaudeCode => format!("{prompt}{PERMISSION_CONFIG_RULE}"),
        crate::runtime::CodingAgent::Codex => {
            let prompt = prompt.replace(ASK_USER_QUESTION_RULE, CODEX_ASK_USER_QUESTION_RULE);
            format!("{prompt}{CODEX_CLI_RULE}{CODEX_SLASH_COMMANDS_RULE}")
        }
    }
}

/// Build the system prompt for Lucidos-source coding-agent threads.
/// Used by both user-initiated coding-agent sessions and LLM-invoked
/// `run_coding_agent` tool calls when editing the Lucidos source tree. Three
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
         The scripts (`scripts/e2e.sh`, `scripts/e2e-browser.sh`, etc.) resolve paths \
         relative to where they live — running them from your worktree uses your worktree's \
         code, which is what you want. `cargo build` and `cargo test` from your worktree \
         compile your worktree's source (Cargo resolves from `Cargo.toml` in cwd). \
         E2E: just run `./scripts/e2e.sh` (full API + browser) or `./scripts/e2e-api.sh` / \
         `./scripts/e2e-browser.sh` for one suite. Each builds the engine + SDK and boots its \
         own session-scoped engine for the disposable `e2e-test` workspace, then cleans up — \
         there is NO separate start step. \
         NEVER run `./scripts/web-dev.sh` (or `run.sh` / `tauri-dev.sh`) from your worktree. \
         Unlike the e2e scripts, `web-dev.sh` starts the MACHINE-GLOBAL gateway — and `-b` \
         stops the user's running one and relaunches it from whatever checkout invoked it. \
         Rooted in your worktree it would outlive your session, adopt every workspace, and \
         serve them all a frontend frozen at your commit, so every later Apply would silently \
         appear to do nothing. It is refused with an actionable message (ADR 0021); do not \
         try to work around the refusal. Restarting the user's workspace is their action, from \
         their own checkout — not yours. \
         All commands run from your worktree directory.\n\n\
         {implementation_plan}\n\n\
         {apply_restart}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. If you abandoned an approach and took a different one, the old \
         edits may still be in the working tree. Discard them with `git checkout -- <file>`. \
         Only intentional changes should remain — stale uncommitted edits get carried into the \
         pending change and cause confusion when the user reviews it.\n\n\
         {commit_cadence}\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         {apply_confirmation}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         SESSION SUMMARY: After hardening completes, output a structured summary of what \
         was implemented in this session. List each change with its status (committed, applied, \
         pending). Include file names and brief descriptions. This is the last thing you output \
         before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}",
        preamble = workspace_preamble(workspace_name),
        branch = branch_name,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        implementation_plan = IMPLEMENTATION_PLAN_RULE,
        apply_restart = APPLY_RESTART_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(true),
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
         workflow wants a differently-named branch (e.g. a ticket branch like `JIRA-1234-...`), \
         RENAME this branch in place with `git branch -m {branch_name} <new-name>` and keep \
         working on it — do NOT `git checkout -b` a separate sibling branch, commit there, and \
         leave this worktree behind. Stranding later commits (e.g. a pre-PR cleanup pass) on a \
         branch this worktree is not on makes the Diff show stale, pre-cleanup work that no longer \
         matches your PR. Whatever branch you finish on MUST be the one this worktree is checked \
         out on — run `git branch --show-current` before you finish to confirm.\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {commit_cadence}\n\n\
         {ask_user_question}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}",
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(false),
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
         {restart_not_rejection}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {commit_cadence}\n\n\
         {ask_user_question}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}",
        branch = branch_name,
        repo = repo_name,
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(false),
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
         {restart_not_rejection}\n\n\
         {implementation_plan}\n\n\
         {apply_restart}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Discard unintentional changes with `git checkout -- <file>`.\n\n\
         {commit_cadence}\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         {apply_confirmation}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}",
        preamble = workspace_preamble(workspace_name),
        branch = branch_name,
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        implementation_plan = IMPLEMENTATION_PLAN_RULE,
        apply_restart = APPLY_RESTART_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(true),
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
         {commit_cadence}\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`.\n\n\
         NO PULL REQUESTS: This workspace's git is local — there is no remote and no PR \
         workflow. Never run `gh pr create`, never `git push` your branch, never tell the \
         user to \"open a PR\" or \"submit a PR\". The engine is the merge mechanism: when \
         the user clicks Apply, your branch lands on the workspace git's `main`.\n\n\
         {ask_user_question}\n\n\
         {apply_confirmation}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         SESSION SUMMARY: Output a structured summary of what was implemented in this \
         session. List each change with a brief description. This is the last thing you \
         output before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or \
         stop, simply say goodbye and finish your response — the Lucidos engine manages \
         your lifecycle. Running `exit` in bash can crash the host application.{process_safety}",
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        app_knowhow = APP_KNOWHOW_RULE,
        process_safety = process_safety_rule(false),
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
         {restart_not_rejection}\n\n\
         When you finish, the user sees a pending *change* in the Apply panel. Apply \
         ff-merges your branch into the workspace git's `main`. No engine restart; no \
         `/harden` (apps own their hardening). Apply emits `AppUiRefreshRequested` if any \
         iframe-bundled file changed.\n\n\
         {app_knowhow}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check \
         for uncommitted changes. Discard unintentional changes with `git checkout -- <file>`.\n\n\
         {commit_cadence}\n\n\
         COMMANDS: Never use /cpa. Just commit with `git add` + `git commit -m \"…\"`.\n\n\
         {ask_user_question}\n\n\
         {apply_confirmation}\n\n\
         {task_lifecycle}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}",
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        task_lifecycle = TASK_LIFECYCLE_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        app_knowhow = APP_KNOWHOW_RULE,
        process_safety = process_safety_rule(false),
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

/// Build a merge prompt for coding-agent sessions.
/// `merge_target` is the branch to merge (e.g. "main" or a feature branch name).
/// `context` is an optional prefix (e.g. "You are running in a temporary merge worktree.").
/// `description` is an optional change description appended at the end.
pub(crate) fn build_merge_prompt(
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
    // App coding-agent threads own their own hardening — their worktree has no
    // `cargo`/`tsc`/`scripts` and their session prompt opts out of `/harden`, so
    // the merge prompt must NOT tell them to run `/harden` or the Lucidos-source
    // test suites (mirrors the `is_app()` skip in `change_ops::apply_change` and
    // `apply_now`). Lucidos-source threads keep the harden + test steps.
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
            3. Run `/harden` to harden the merged code\n\
            4. Run `cargo test -p lucidos-engine` and `cd crates/lucidos-app && npm test` to verify\n\
            5. Fix any test failures before finishing\n\n\
            If any conflict is ambiguous, ask the user before proceeding.",
            merge_target,
        ));
    }
    if let Some(desc) = description {
        prompt.push_str(&format!("\n\nThe change being applied: {}", desc));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Lucidos-source merge prompt keeps the `/harden` + test steps — those
    /// commands resolve from a Lucidos-source worktree and gate the merge.
    #[test]
    fn merge_prompt_lucidos_source_keeps_harden_and_tests() {
        let prompt = build_merge_prompt("main", None, Some("desc"), false);
        assert!(prompt.contains("git merge main"));
        assert!(prompt.contains("/harden"));
        assert!(prompt.contains("cargo test -p lucidos-engine"));
        assert!(prompt.contains("desc"));
    }

    /// Regression: the *app* merge prompt must NOT tell the app session to run
    /// `/harden` or the Lucidos-source test suites — app worktrees have none of
    /// that tooling. This is the CC-assisted (diverged-main) counterpart of the
    /// `apply_now` pre-merge harden-gate skip; without it, an app apply whose
    /// `main` diverged still injected `/harden` + `cargo test` into the app
    /// session (the "Create App Demo Video" / `demo-director` bug).
    #[test]
    fn merge_prompt_app_omits_harden_and_tests() {
        let prompt = build_merge_prompt("main", None, Some("desc"), true);
        assert!(
            prompt.contains("git merge main"),
            "still a real merge prompt"
        );
        assert!(
            prompt.contains("desc"),
            "still carries the change description"
        );
        assert!(
            !prompt.contains("/harden"),
            "app merge prompt must not mention /harden; got: {prompt}",
        );
        assert!(
            !prompt.contains("cargo test"),
            "app merge prompt must not mention cargo test; got: {prompt}",
        );
        assert!(
            !prompt.contains("npm test"),
            "app merge prompt must not mention npm test; got: {prompt}",
        );
    }

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
        // The plan-marker vocabulary. External repos and app worktrees are
        // exempt from the gate and have no `docs/plans/` convention, so
        // `IMPLEMENTATION_PLAN_RULE` is left out of their prompts entirely
        // (asserted separately, by the `exempt` list in
        // `lucidos_source_prompts_carry_implementation_plan_rule_for_both_backends`;
        // this list is checked against the external-repo prompts only).
        // Neither token was named here until 2026-08-04, and the shared
        // (environment-generic) `ASK_USER_QUESTION_RULE` briefly grew a
        // "revise the plan file, then run `lucidos planned approve`" clause
        // that rode into external prompts through the back door: the same
        // leak `APPLY_CONFIRMATION_NOTE` was split out to prevent, via a
        // different constant. Keep marker machinery in the plan rule.
        "lucidos planned",
        "docs/plans/",
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
        let manifest = r#"{"name":"Habit Tracker","icon":"target"}"#;
        let prompt = app_worktree_system_prompt(
            "claude-code/app/habit-tracker/20260527-100000-abc123",
            "dev",
            "habit-tracker",
            manifest,
        );
        assert!(
            prompt.contains("`habit-tracker`"),
            "must name the app id so CC knows which folder it owns",
        );
        assert!(
            prompt.contains("dev"),
            "must name the workspace so cross-workspace context is clear",
        );
        assert!(
            prompt.contains("claude-code/app/habit-tracker/20260527-100000-abc123"),
            "must surface the branch name for the user-side Apply chip",
        );
        assert!(
            prompt.contains("Habit Tracker"),
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
            "claude-code/app/habit-tracker/20260527-100000-abc123",
            "dev",
            "habit-tracker",
        );
        assert!(prompt.contains("`habit-tracker`"));
        assert!(prompt.contains("claude-code/app/habit-tracker/20260527-100000-abc123"));
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
        let prompt = external_repo_system_prompt("Acme", "JIRA-1879-fix", "origin/main");
        assert!(
            prompt.contains("git branch -m JIRA-1879-fix"),
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
    fn recovery_prompts_carry_restart_not_rejection_note() {
        // Every flavor of post-restart resume must tell the agent that a denial /
        // interrupted tool call in its transcript is a restart artifact, not the
        // user rejecting its approach — otherwise the resumed session changes
        // course on a phantom rejection.
        let cases: &[(&str, String)] = &[
            (
                "recovery_system_prompt",
                recovery_system_prompt("feature/x", "dev"),
            ),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme", "feature/x"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("feature/x", "dev", "habit-tracker"),
            ),
        ];
        for (label, prompt) in cases {
            for needle in [
                "RESTART CONTEXT — NOT A REJECTION",
                "NOT by the user rejecting",
                "Do not abandon or rework your approach",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must carry the restart-not-rejection note (missing: {needle:?})",
                );
            }
        }
        // The note must stay repo-generic so it is safe in the external-repo prompt.
        assert_no_lucidos_only_tokens(
            &external_repo_recovery_system_prompt("Acme", "feature/x"),
            "external_repo_recovery_system_prompt (with restart note)",
        );
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
    fn coding_agent_prompts_encourage_regular_reviewable_commits() {
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
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("feature/x", "dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("feature/x", "dev", "habit-tracker"),
            ),
        ];

        for (label, prompt) in cases {
            for needle in [
                "COMMIT CADENCE",
                "Commit completed, coherent slices of work as you go",
                "Do not commit after every tiny edit",
                "Diff view and recovery state stay current",
                "known-broken work",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must keep regular-commit guidance (`{needle}`)",
                );
            }
        }

        assert!(
            !conflict_resolution_system_prompt().contains("COMMIT CADENCE"),
            "merge-conflict sessions should make the single merge commit after all conflicts are resolved",
        );
    }

    #[test]
    fn coding_agent_prompts_warn_background_processes_die_at_turn_end() {
        // A coding-agent session is a per-turn subprocess: at idle the engine
        // tears down its whole process group, so a `run_in_background` job left
        // running as the turn ends is killed, and nothing wakes the agent when
        // it would have finished (only the chat agent's `run_bash_background`
        // has that wake path). Without this guidance the agent trusts its native
        // Bash-tool "runs across turns, re-invokes you" contract and loops — the
        // real DMG-build thread restarted the build three times and never
        // produced an artifact. Every chat-style prompt must carry the warning;
        // the merge-conflict prompt (no builds) deliberately omits it.
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
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("feature/x", "dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("feature/x", "dev", "habit-tracker"),
            ),
        ];
        for (label, prompt) in cases {
            for needle in [
                "BACKGROUND PROCESSES DON'T SURVIVE A TURN",
                "run_in_background",
                // The two workable patterns must both survive a rewrite:
                // foreground, or wait-to-completion inside the same turn.
                "FOREGROUND",
                "WITHIN THE SAME TURN",
                // …and so must the half that makes the wait cheap. Without the
                // 600000 ms ceiling named for BOTH shapes, "wait inside the
                // turn" degrades back into ticking on the 120000 ms Bash
                // default — ~20 round-trips for one release build.
                "WAIT IN AS FEW CALLS AS YOU CAN",
                "`timeout: 600000`",
                "`block: true`",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must warn that background processes die at turn end (`{needle}`)",
                );
            }
        }

        // The rule lives in the shared base, so both backends inherit it — the
        // appended backend section can't strip it.
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            let full = append_backend_rules(worktree_system_prompt("feature/x", "dev"), agent);
            assert!(
                full.contains("BACKGROUND PROCESSES DON'T SURVIVE A TURN"),
                "worktree_system_prompt must keep the background-process rule for {:?}",
                agent,
            );
        }

        // Merge-conflict sessions don't run builds — the rule would be noise.
        assert!(
            !conflict_resolution_system_prompt()
                .contains("BACKGROUND PROCESSES DON'T SURVIVE A TURN"),
            "merge-conflict prompt should omit the background-process rule",
        );
    }

    /// Every prompt flavor, built the way the real spawn builds it (before
    /// `append_backend_rules`). Shared by the guards for rules injected at that
    /// chokepoint, which must reach all of them for both backends.
    fn all_prompt_flavors() -> Vec<(&'static str, String)> {
        vec![
            ("worktree", worktree_system_prompt("feature/x", "dev")),
            (
                "external_repo",
                external_repo_system_prompt("Acme", "feature/x", "origin/main"),
            ),
            ("recovery", recovery_system_prompt("feature/x", "dev")),
            (
                "external_repo_recovery",
                external_repo_recovery_system_prompt("Acme", "feature/x"),
            ),
            (
                "app_worktree",
                app_worktree_system_prompt("feature/x", "dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery",
                app_worktree_recovery_system_prompt("feature/x", "dev", "habit-tracker"),
            ),
            (
                "conflict_resolution",
                conflict_resolution_system_prompt().to_string(),
            ),
        ]
    }

    #[test]
    fn coding_agent_prompts_tell_agent_its_reasoning_is_not_visible() {
        // Regression guard for the "Caption copy: do the six lines above work?"
        // card whose six lines never rendered: the model drafted them in its
        // reasoning (display-omitted / signature-only for the current models, and
        // unavailable through the CC CLI we drive) and then referenced them as if
        // shown. Every coding-agent prompt (every flavor, both backends) must
        // tell the agent its reasoning is not shown so it puts must-see content in
        // a visible message. The rule is injected at the shared
        // `append_backend_rules` chokepoint, so build each flavor and run it
        // through that (the same way the real spawn does).
        let flavors = all_prompt_flavors();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &flavors {
                let full = append_backend_rules(base.clone(), agent);
                for needle in [
                    "YOUR REASONING IS NOT SHOWN TO THE USER",
                    "MUST go in a visible assistant message",
                    "the six lines above",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must tell the agent its reasoning is not shown (`{needle}`)",
                    );
                }
            }
        }
        // The backend-independent prepend must not have broken the Codex
        // ask-rule swap that `append_backend_rules` performs after it.
        let codex = append_backend_rules(
            worktree_system_prompt("feature/x", "dev"),
            crate::runtime::CodingAgent::Codex,
        );
        assert!(
            codex.contains("ask_user_question` tool (on the `lucidos` MCP"),
            "Codex prompt must still swap in the MCP ask_user_question rule",
        );
    }

    /// Regression guard for the question card that asked "Change 99da1708 is
    /// docs-only and its plan is separate from it. What do you want first?"
    /// with an option labelled "Apply 99da1708 now". A change id names nothing
    /// the user can see, so the card was unanswerable. Rides the same
    /// `append_backend_rules` chokepoint as the reasoning rule, so every flavor
    /// and both backends must carry it.
    #[test]
    fn coding_agent_prompts_forbid_raw_ids_and_shas_in_user_facing_text() {
        let flavors = all_prompt_flavors();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &flavors {
                let full = append_backend_rules(base.clone(), agent);
                for needle in [
                    "NAME THINGS THE WAY THE USER SEES THEM, NEVER A RAW ID OR SHA",
                    "NEVER put one in an assistant message, in a question, or in an option label",
                    "a commit by its subject line",
                    "never shas",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must forbid raw ids in user-facing text (`{needle}`)",
                    );
                }
                // Presentation-only. Without the carve-outs the agent
                // over-corrects: it stops passing shas to git and ids to tools,
                // and it drops the `lucidos spawn-thread` link that is the
                // user's only way to open a thread it started.
                assert!(
                    full.contains("git commands, tool arguments"),
                    "{label} ({agent:?}) must keep ids legal where they are required",
                );
                assert!(
                    full.contains("markdown link TARGET is not prose")
                        && full.contains("[title](thread:<ws>/<uuid>)"),
                    "{label} ({agent:?}) must exempt markdown link targets",
                );
            }
        }
    }

    /// Codex has no slash-command runtime, yet prompts across every flavor
    /// tell it to "run `/harden`" — the shared [`HARDENING_RULE`], the
    /// merge-conflict prompt's harden step, and the engine's auto-harden
    /// follow-up ("Run /harden now."). Without an explicit mapping it must
    /// guess, and it guessed badly in practice: 17% of Codex changes hit
    /// Apply unhardened vs 0.6% for CC. The Codex arm of
    /// `append_backend_rules` therefore defines the slash-command → playbook
    /// file mapping once, on every prompt flavor (a hardening-session
    /// override and a merge prompt need it just as much as a fresh turn).
    #[test]
    fn codex_prompts_map_slash_commands_to_playbook_files() {
        let flavors: &[(&str, String)] = &[
            ("worktree", worktree_system_prompt("feature/x", "dev")),
            ("recovery", recovery_system_prompt("feature/x", "dev")),
            (
                "conflict_resolution",
                conflict_resolution_system_prompt().to_string(),
            ),
            // Stand-in for the hardening-session `system_prompt_override` —
            // backend rules ride overrides through the same chokepoint.
            ("override", "HARDENING SESSION: run /harden".to_string()),
        ];
        for (label, base) in flavors {
            let codex = append_backend_rules(base.clone(), crate::runtime::CodingAgent::Codex);
            for needle in [
                "SLASH COMMANDS:",
                ".claude/commands/harden.md",
                "lucidos hardened query",
            ] {
                assert!(
                    codex.contains(needle),
                    "{label} (Codex) must map slash commands to playbook files (`{needle}`)",
                );
            }
        }
        // CC has a real slash-command runtime — the mapping would be noise.
        let cc = append_backend_rules(
            worktree_system_prompt("feature/x", "dev"),
            crate::runtime::CodingAgent::ClaudeCode,
        );
        assert!(
            !cc.contains("SLASH COMMANDS:"),
            "CC prompt must not carry the Codex slash-command mapping",
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
                !prompt
                    .to_lowercase()
                    .contains("default to `askuserquestion`"),
                "{label} must keep the AskUserQuestion rule as an unconditional imperative \
                 for DECISIONS — softer phrasing (\"default to\") let CC slip back to \
                 plaintext for the choice-shaped questions it should route through the tool",
            );
            // The rule must FORBID post-work confirmations, not encourage them. A
            // held-open question parks the thread in `waiting_for_user_answer`, which
            // stalls hand-off — and a visual result can't be judged until it's landed.
            // The old wording told CC to turn end-of-turn "does this look complete?"
            // into a button question, which caused the "cant apply when you ask
            // question" deadlock. "does this look complete" must now appear as a DON'T
            // example. This is the ENVIRONMENT-GENERIC half — the Apply-specific
            // sharpening (which names the Apply button) is asserted separately and is
            // intentionally absent from external-repo prompts.
            assert!(
                prompt.contains("does this look complete"),
                "{label} must name the concrete post-work confirmation it must NOT ask",
            );
            for needle in [
                "DO NOT ask a confirmation question",
                "Ask to DECIDE, never to CONFIRM finished work",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must forbid post-work confirmations (missing: {needle:?})",
                );
            }
            assert!(
                prompt.contains("Mid-stream decision questions"),
                "{label} must keep mid-stream DECISION questions allowed — those don't \
                 block anything (no finished change yet); only post-work confirmations \
                 are forbidden. The distinction is the whole fix, so pin it",
            );
            // Plan approval is the case the DON'T half over-reached and swallowed:
            // a committed plan reads as work already done, so the agent asked in
            // prose and the thread sat idle until the user typed "approve"
            // (2026-08-02). The carve-out must survive, with the concrete option
            // pair that makes it actionable (the tool needs 2-4 options, so a lone
            // `Approve` is not expressible).
            for needle in ["APPROVE A PLAN OR AN", "`Approve` and `Request changes`"] {
                assert!(
                    prompt.contains(needle),
                    "{label} must carve plan approval OUT of the no-confirmations rule \
                     and name its option pair (missing: {needle:?})",
                );
            }
            // ...but that pair is a FLOOR, not a fixed shape. Stated
            // unconditionally it produced a live three-option card
            // (`Approve` / `Frontend only` / `Request changes`) whose last
            // button meant only "I will type what I want changed", which the
            // NEVER AUTHOR AN "OTHER" OPTION paragraph in the same rule bans
            // (2026-08-04). A real fork satisfies the two-option minimum on its
            // own, so `Request changes` must be dropped rather than pushed to a
            // third slot.
            // Needle is ask-rule-specific on purpose: `IMPLEMENTATION_PLAN_RULE`
            // states the same floor in its own words ("takes the second slot"),
            // so a bare "FLOOR" would pass on that rule alone.
            assert!(
                prompt.contains("make that fork the second option"),
                "{label} must state that the approval option pair is a FLOOR, not a fixed \
                 shape, so a real fork replaces `Request changes` instead of joining it",
            );
            // Without this, the floor rule leaves the agent unsure whether
            // anything but a literal `Approve` counts as approval. Kept
            // ENVIRONMENT-GENERIC on purpose: what to DO about it (revise the
            // plan file, re-commit, `lucidos planned approve`) belongs to
            // `IMPLEMENTATION_PLAN_RULE`, which external-repo and app-worktree
            // prompts don't carry. Do not move that clause up here.
            assert!(
                prompt.contains("Picking a fork is still an approval"),
                "{label} must say a fork answer approves that variant rather than rejecting \
                 the plan (without naming the plan marker, which is Lucidos-source only)",
            );
            // A prose question ends the turn, so the thread looks finished and
            // nothing tells the user someone is waiting. Only the tool call
            // parks it in `WaitingForUserAnswer`, which drives the
            // needs-attention badge, Review routing, and the notification
            // trigger. Pin the general form, not just the plan-approval case.
            assert!(
                prompt.contains("WHY THE TOOL AND NOT PROSE"),
                "{label} must say that only the tool marks the thread as needing attention, \
                 so a prose question reads as a finished turn",
            );
            assert!(
                prompt.contains("NEVER parallel-call"),
                "{label} must forbid parallel-calling `AskUserQuestion` alongside other \
                 tools (see ASK_USER_QUESTION_RULE)",
            );
            // CC's own tool description promises an "Other" option is "provided
            // automatically". Lucidos provides none, and every option is a
            // label, so an agent-authored "Other, I'll type it" hands that
            // phrase back as the user's answer. The prompt must contradict the
            // upstream promise outright, not stay silent about it.
            for needle in [
                "NEVER AUTHOR AN \"OTHER\" OPTION",
                "In Lucidos it is NOT",
                "prompt textarea",
                "Cancel dismisses the question",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must ban the text-entry escape option and name the real escapes \
                     (missing: {needle:?})",
                );
            }
            assert!(
                !prompt.contains("escape the tool adds for them"),
                "{label} must not claim the tool auto-adds an \"Other\" escape. Lucidos \
                 renders exactly the options passed",
            );
            // Banning the text-entry escape must not read as banning every
            // opt-out: "None of these" is a decision the agent can act on.
            assert!(
                prompt.contains("None of these") && prompt.contains("still welcome"),
                "{label} must keep a meaningful opt-out option legal",
            );
        }
    }

    /// The Apply-specific sharpening (`APPLY_CONFIRMATION_NOTE`) names the Apply
    /// button — true only in Apply-based worktrees. It MUST reach the four
    /// Apply-based prompts (Lucidos-source worktree + recovery, app worktree +
    /// recovery) for BOTH backends (the Codex `ASK_USER_QUESTION_RULE` swap must
    /// leave the separate note intact), and MUST NOT leak into external-repo
    /// prompts, whose push/PR workflow has no Apply — telling that agent to "let
    /// the user click Apply" could make it stop after committing without pushing
    /// or opening a PR (the Codex-review finding this test pins).
    #[test]
    fn apply_confirmation_note_scoped_to_apply_based_prompts() {
        let apply_based: &[(&str, String)] = &[
            ("worktree", worktree_system_prompt("feature/x", "dev")),
            ("recovery", recovery_system_prompt("feature/x", "dev")),
            (
                "app_worktree",
                app_worktree_system_prompt("feature/x", "dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery",
                app_worktree_recovery_system_prompt("feature/x", "dev", "habit-tracker"),
            ),
        ];
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in apply_based {
                let full = append_backend_rules(base.clone(), agent);
                assert!(
                    full.contains("BLOCKS the Apply button"),
                    "{label} ({agent:?}) must carry the Apply-specific confirmation note \
                     — the Codex ask-rule swap must not strip the separate note",
                );
            }
        }

        // External-repo prompts must NOT mention Apply / change-proposal — they
        // push and open PRs. Guards the exact leak the split fixes.
        let external: &[(&str, String)] = &[
            (
                "external_repo",
                external_repo_system_prompt("Acme", "feature/x", "origin/main"),
            ),
            (
                "external_repo_recovery",
                external_repo_recovery_system_prompt("Acme", "feature/x"),
            ),
        ];
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in external {
                let full = append_backend_rules(base.clone(), agent);
                for banned in [
                    "BLOCKS the Apply button",
                    "click Apply",
                    "the change be proposed",
                ] {
                    assert!(
                        !full.contains(banned),
                        "{label} ({agent:?}) must NOT carry Apply-specific wording \
                         (found: {banned:?}) — external repos use push/PR, not Apply",
                    );
                }
            }
        }
    }

    /// The implementation-plan rule must live in the shared Lucidos-source
    /// base so it reaches BOTH Claude Code AND Codex (Codex has no PreToolUse
    /// hook, so the prompt + Apply floor are its only enforcement). It must NOT
    /// appear in external-repo or app prompts (no `docs/plans/` convention
    /// there). The marker CLI must be named so the agent knows how to satisfy
    /// the gate for a local fix.
    #[test]
    fn lucidos_source_prompts_carry_implementation_plan_rule_for_both_backends() {
        let lucidos_cases: &[(&str, String)] = &[
            (
                "worktree_system_prompt",
                worktree_system_prompt("feature/x", "dev"),
            ),
            (
                "recovery_system_prompt",
                recovery_system_prompt("feature/x", "dev"),
            ),
        ];
        for (label, base) in lucidos_cases {
            for needle in [
                "IMPLEMENTATION PLAN:",
                "implementation-plan",
                "lucidos planned mark --simple",
                "lucidos planned approve",
                "docs/plans/",
                // The approval must be ASKED with the question tool, not written
                // as prose. Told only to "present the plan and wait", the agent
                // ended the turn and the thread sat idle until the user typed
                // "approve" by hand (2026-08-02). The tool is named indirectly
                // ("the ASKING USERS section") because the two backends call it
                // different things and `append_backend_rules` swaps only that
                // section, never this rule.
                "ASK FOR APPROVAL WITH THE QUESTION TOOL",
                "`Approve` and `Request changes`",
                // The pair is a FLOOR: a plan that offers a real fork puts the
                // fork in the second slot instead of pushing `Request changes`
                // to a third, where it would mean only "I will type what I want
                // changed" (2026-08-04). And a fork answer is an approval, so
                // the rule that owns `lucidos planned approve` has to say the
                // agent may flip the marker after revising the plan to match.
                "that fork takes the second slot",
                "Picking a fork is an approval too",
            ] {
                assert!(
                    base.contains(needle),
                    "{label} must carry the implementation-plan rule (`{needle}`)",
                );
            }
            // The indirection only works if the section it points at exists in
            // the same prompt. Pin the pointer's target so a rename of the
            // ASKING USERS heading can't leave the plan rule dangling.
            assert!(
                base.contains("ASKING USERS:"),
                "{label} names \"the ASKING USERS section\" for the approval tool, so \
                 that section must be in the same prompt",
            );
            // Both backends inherit it: the rule is in the shared base, so the
            // appended backend section can't strip it.
            for agent in [
                crate::runtime::CodingAgent::ClaudeCode,
                crate::runtime::CodingAgent::Codex,
            ] {
                let full = append_backend_rules(base.clone(), agent);
                assert!(
                    full.contains("IMPLEMENTATION PLAN:"),
                    "{label} must keep the implementation-plan rule for {:?}",
                    agent,
                );
            }
        }

        // External repos and app worktrees have no docs/plans convention —
        // the rule must NOT leak into their prompts.
        let exempt: &[(&str, String)] = &[
            (
                "external_repo_system_prompt",
                external_repo_system_prompt("Acme", "feature/x", "origin/main"),
            ),
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("feature/x", "dev", "habit-tracker", "{}"),
            ),
        ];
        for (label, prompt) in exempt {
            assert!(
                !prompt.contains("IMPLEMENTATION PLAN:"),
                "{label} must NOT carry the implementation-plan rule (no docs/plans there)",
            );
        }
    }

    /// Plan approval must reach the user as a button question on BOTH backends.
    /// The failure this pins: `IMPLEMENTATION_PLAN_RULE` said only "present the
    /// plan and wait for their approval", while the ASKING USERS rule in the
    /// same prompt forbade confirmations about "work you've ALREADY done". A
    /// committed plan looks exactly like already-done work, so the agent
    /// classified approval as forbidden and asked in prose. The thread then sat
    /// idle for 20 minutes until the user typed "approve" by hand (2026-08-02).
    ///
    /// Each backend must name its OWN tool: `append_backend_rules` swaps the
    /// whole `ASK_USER_QUESTION_RULE` for the Codex variant, so a CC tool name
    /// leaking into a Codex prompt would send it after a tool it cannot call.
    #[test]
    fn plan_approval_is_carved_out_of_the_no_confirmations_rule_on_both_backends() {
        let base = worktree_system_prompt("feature/x", "dev");
        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        let codex = append_backend_rules(base, crate::runtime::CodingAgent::Codex);

        // Both backends carry the carve-out and the same option pair, so the
        // two rules, the cc-plan-gate deny message and the skill stay
        // describable in one sentence.
        for (label, prompt) in [("claude-code", &cc), ("codex", &codex)] {
            assert!(
                prompt.contains("`Approve` and `Request changes`"),
                "{label} must name the same approval option pair as the skill and the \
                 cc-plan-gate deny message",
            );
            // And describe it the same way: as a FLOOR. Both backends must say
            // a real fork takes the second slot INSTEAD of `Request changes`,
            // because the Codex swap replaces the whole CC rule and would
            // otherwise keep prescribing an unconditional pair. Stated
            // unconditionally, it produced a three-option card whose last
            // button was the dead-end shape the same rule bans (2026-08-04).
            for needle in [
                "make that fork the second option",
                "Picking a fork is still an approval",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must describe the option pair as a floor a real fork replaces, \
                     and say a fork answer is an approval (missing: {needle:?})",
                );
            }
            // The general lesson, not just the plan-approval instance: a prose
            // question ends the turn, so the thread reads as finished and the
            // user is never told anyone is waiting. Only a tool call parks it
            // in `WaitingForUserAnswer`, which is what
            // `thread_lifecycle::is_attention_needing` keys on.
            assert!(
                prompt.contains("WHY THE TOOL AND NOT PROSE"),
                "{label} must explain that only a tool call marks the thread as needing \
                 attention; prose leaves it looking finished",
            );
        }

        assert!(
            cc.contains("APPROVE A PLAN OR AN"),
            "Claude Code prompt must exempt plan approval from the no-confirmations rule",
        );
        assert!(
            cc.contains("`AskUserQuestion` tool"),
            "Claude Code prompt must name its own question tool",
        );
        assert!(
            codex.contains("Approving a plan BEFORE you implement it"),
            "Codex prompt must exempt plan approval from the no-confirmations rule \
             (the swap to CODEX_ASK_USER_QUESTION_RULE must carry the carve-out too)",
        );
        assert!(
            codex.contains("`ask_user_question` tool"),
            "Codex prompt must name the MCP tool it can actually call",
        );
        assert!(
            !codex.contains("`AskUserQuestion` tool"),
            "Codex has no `AskUserQuestion` tool; the swap must leave no CC tool name behind",
        );
    }

    #[test]
    fn lucidos_worktree_prompt_keeps_harden_and_cpa_guidance() {
        // Don't accidentally strip these from the Lucidos-repo prompt while
        // tightening the external one — `/harden` and `/cpa` are real here.
        let prompt = worktree_system_prompt("feature/x", "dev");
        assert!(
            prompt.contains("/harden"),
            "Lucidos prompt must keep /harden guidance"
        );
        assert!(
            prompt.contains("/cpa"),
            "Lucidos prompt must keep /cpa guidance"
        );
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

    /// The Codex teaching section must be applied for Codex and ONLY for
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
            !codex.contains("AskUserQuestion"),
            "Codex prompt must not carry Claude Code's native AskUserQuestion rule; \
             Codex uses the lucidos MCP ask_user_question tool"
        );
        assert!(
            !codex.contains("request_user_input"),
            "Codex prompt must not point at Codex's plan-only request_user_input helper"
        );
        assert!(
            codex.contains("NEVER parallel-call `ask_user_question` alongside other tools"),
            "Codex prompt must keep the no-parallel-call question safety rule with the \
             available MCP tool name"
        );
        // The Codex rule REPLACES the CC one wholesale, so every teaching that
        // matters has to be restated here or Codex simply never sees it. The
        // dead-end "Other" option is one of those: the card renders exactly the
        // options passed, and tapping one returns its label as the answer.
        for needle in [
            "NEVER AUTHOR AN \"OTHER\" OPTION",
            "prompt textarea",
            "Cancel dismisses the question",
            "None of these",
        ] {
            assert!(
                codex.contains(needle),
                "the Codex question rule must carry the no-escape-hatch-option ban too \
                 (missing: {needle:?})",
            );
        }
        let codex_base = base.replace(ASK_USER_QUESTION_RULE, CODEX_ASK_USER_QUESTION_RULE);
        assert!(
            codex.starts_with(&codex_base),
            "Codex backend rules must preserve the base prompt while replacing only the \
             backend-specific user-question rule",
        );

        // CC gets its OWN backend section (the permission-config rule), not the
        // Codex CLI teaching — so it appends to the base rather than passing
        // through unchanged, but it must never duplicate the Codex section.
        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        assert!(
            cc.starts_with(&base),
            "backend rules must append, not replace, the worktree prompt",
        );
        assert!(
            !cc.contains("lucidos data write"),
            "the CC prompt must not duplicate the Codex CLI teaching",
        );
    }

    /// The permission-config rule (`--allowedTools` / `~/.lucidos/cc-allowed-tools`
    /// mechanics) is Claude-Code-only and must be appended for CC and ONLY for
    /// CC — mirroring how [`CODEX_CLI_RULE`] is Codex-only. Codex
    /// permissions surface through its own sandbox + approval-policy model
    /// (approval cards raised by the app-server), so the CC mechanics are
    /// misleading noise (and wasted tokens) on a Codex session. The shared
    /// base prompt must carry NEITHER backend's section: `append_backend_rules`
    /// is the single split point.
    #[test]
    fn permission_config_rule_is_claude_code_only() {
        let base = worktree_system_prompt("feature/x", "dev");
        // The base must not embed the permission-config rule — it is appended
        // per-backend, so a base that already carried it would leak the CC-only
        // mechanics into Codex via append-on-top.
        for needle in ["PERMISSION CONFIG:", "--allowedTools", "cc-allowed-tools"] {
            assert!(
                !base.contains(needle),
                "shared base prompt must not embed `{needle}` — the permission-config rule \
                 is appended only by append_backend_rules (Claude Code arm)",
            );
        }

        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        for needle in [
            "PERMISSION CONFIG:",
            "--allowedTools",
            "~/.lucidos/cc-allowed-tools",
        ] {
            assert!(
                cc.contains(needle),
                "Claude Code prompt must carry the full permission-config rule (`{needle}`)",
            );
        }

        let codex = append_backend_rules(base, crate::runtime::CodingAgent::Codex);
        for needle in ["PERMISSION CONFIG:", "--allowedTools", "cc-allowed-tools"] {
            assert!(
                !codex.contains(needle),
                "Codex prompt must NOT carry the CC-only permission-config rule (`{needle}`) — \
                 Codex permissions surface via its sandbox + approval-policy model",
            );
        }
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
