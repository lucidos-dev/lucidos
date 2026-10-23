use crate::engine::cache_ttl::CacheTtl;

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
         For e2e, run `./scripts/e2e.sh` directly. It runs on GitHub by default; \
         with `--local` it builds and boots its own session-scoped engine and \
         cleans up after itself. Do NOT pre-start with \
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

/// Lucidos-repo-only rule: a session's knowledge belongs in git, never in the
/// coding agent's per-user memory directory. Scoped to the two Lucidos-source
/// flavors on purpose. An external repo is somebody else's working agreement,
/// and an app worktree is the user's own workspace git.
///
/// Both backends carry it, because the standard is about where the fact lands
/// rather than which agent wrote it: a Codex session reading the repo is the
/// reader a memory file hides the fact from.
const NO_MEMORY_FILES_RULE: &str = "NEVER WRITE A CODING-AGENT MEMORY FILE: Do not write to \
    your agent's per-user memory directory (for Claude Code that is \
    `$CLAUDE_CONFIG_DIR/projects/<cwd>/memory/`, a `MEMORY.md` plus one file per fact). \
    Nothing else reads it: not Codex, not the user, not the next session on another machine. \
    Knowledge lives in git instead, committed in the same change so it merges. A convention \
    for agents goes in `CLAUDE.md` or `.claude/rules/`; a workspace fact goes in the \
    workspace's own knowhow, written with `lucidos data write knowhow/<topic>/<name>.md`.";

/// Apply/restart rule shared across Lucidos-repo CC system prompts. The
/// file-type list and the hard ban must stay in sync with
/// `engine::git_ops::files_require_restart` (the truth) and the
/// WaitingBanner button label (the user-visible signal). The regression
/// test `lucidos_prompts_carry_full_apply_restart_rule` documents the
/// failure mode that motivated the hard ban.
const APPLY_RESTART_RULE: &str = "APPLY/RESTART: After your session ends, your commits sit \
    as a pending change on this thread. The user explicitly clicks Apply to merge your branch into \
    main — nothing happens automatically. The button label is \"Apply\" (no restart needed) or \
    \"Apply*\" (restart needed); the engine derives this from the touched files. ANY \
    of these triggers restart: a non-test `.rs` file, `Cargo.toml`, `Cargo.lock`, a `.sql` \
    migration under `migrations/`, an SDK bundle source under `packages/lucidos-sdk/`, any \
    non-test, non-doc file in `crates/lucidos-engine/`, `crates/lucidos-gateway/` or `crates/lucidos-cli/` \
    (e.g. `crates/lucidos-engine/src/api/sdk_iframe.css`, `sdk_iframe_audio.js`), or a file a \
    binary embeds (`crates/lucidos-app/src/styles/global/shared-components.css`, `index.html`, \
    the fonts). Frontend-only edits (TypeScript/CSS outside those) do NOT trigger restart.\n\n\
    Apply never restarts Lucidos, even when you run `lucidos changes apply` yourself. It builds \
    the new version in the background; the user then taps \"Switch to new version\" to restart \
    onto it. Never tell the user an apply restarts Lucidos.\n\n\
    DO NOT comment on restart status in your session summary or anywhere else — do not write \
    \"no restart required\", \"restart required\", \"just a code rebuild\", or any equivalent. \
    The button label is the source of truth and the user already sees it. If your intuition \
    disagrees with the button, your intuition is wrong; `files_require_restart` \
    (in `crates/lucidos-engine/src/engine/git_ops/restart_detection.rs`) is authoritative.";

/// Hardening reminder shared across all Lucidos-repo CC system prompts. The
/// /harden skill itself runs the test suites and iterates on failure — keep
/// this text in sync with `.claude/commands/harden.md` Phase 4.5.
///
/// This is the SOLE statement of the rule to a session. Running `/harden` is
/// session truth, so the engine owns it outright under the split in
/// `docs/agent-config.md` § Which surface owns a rule. The last three sentences
/// exist nowhere else, so do not trim them as redundant. What stays in
/// `CLAUDE.md` is the repo-side fact that `.claude/hooks/pre-push.sh` enforces
/// the marker, which binds a hand-run `claude` that never sees this text.
const HARDENING_RULE: &str = "HARDENING: Once your implementation is complete and committed, \
    you MUST run `/harden`. The one exception is a branch landing only plan files \
    (`git_ops::is_plan_file`): Apply never hardens it, so skip `/harden` there. Every other \
    change runs it, even a docs-only, CSS-only, comment-only, or seemingly trivial one. Do \
    not rationalize skipping it (\"too small to harden\", \"nothing to test\", \"just a \
    wording tweak\"). The skill decides what to check and which suites to run, and iterates \
    until they pass. \
    The harden marker only exists if you actually invoke `/harden`, and without it the user \
    pays the wait when they click Apply. Run it ONCE, over the whole finished batch of \
    commits: hardening mid-work re-runs the same suites for nothing and says nothing about \
    the commits that follow it. If Phase 0 reports ALREADY_HARDENED, say so and stop. Never \
    tell the user you are postponing, deferring, or skipping it. There is no such option in \
    the system, and Apply runs hardening synchronously when the marker is missing, so a \
    postponement you announce is a confusing lie.";

/// Implementation-planning rule shared by the two Lucidos-source prompts
/// (`worktree_system_prompt`, `recovery_system_prompt`). Lives in the shared
/// base, NOT in a backend section, so it reaches both Claude Code and Codex.
///
/// This is the soft, prospective half of enforcement. The hard halves are the
/// Claude-Code `cc-plan-gate` PreToolUse hook and the Apply floor. Codex has no
/// PreToolUse hook, so for Codex this rule plus the Apply floor are the whole
/// enforcement. Keep in sync with the `implementation-plan` skill and
/// `lucidos planned`.
///
/// The bounded security-fix carve-out is deliberately NOT stated here. It cost
/// ~500 bytes on every request of every Lucidos-source session, for a lane only
/// an unattended security run takes. This prompt is ceiling-gated by
/// `every_prompt_flavor_stays_under_its_size_ceiling`, which is what said so. It lives where it is
/// free: the `cc-plan-gate` deny message, which arrives exactly when an edit is
/// blocked, the skill's own section, and the nightly's spawn intent. A session
/// that never learns the lane exists writes a plan and reports blocked, which
/// is the safe outcome, not a failure.
const IMPLEMENTATION_PLAN_RULE: &str = "IMPLEMENTATION PLAN: Before your FIRST code edit, decide \
    whether this is complex work — ADR- or design-thread-backed, cross-layer, any routing / \
    topology / storage / security / migration / process change, or anything beyond a local bug \
    fix. If it is, produce an implementation plan FIRST: run the `implementation-plan` skill \
    (`.claude/skills/implementation-plan/SKILL.md`) — it turns the prompt, any grill/design \
    thread, ADRs, and code reconnaissance into `docs/plans/<date>-<slug>.md` and records a \
    PROPOSED plan marker via `lucidos planned mark --plan <path>`. A proposed plan does NOT \
    unblock editing: ASK FOR APPROVAL WITH THE QUESTION TOOL named in the ASKING USERS section, \
    offering `Approve` and `Request changes` in the shape that section gives. A message that also \
    calls a tool never reaches the user, so the card's question carries a short markdown summary \
    of the plan, ask last. Offer the options the skill's approval checkpoint prints, which may \
    swap this pair for `Implement in a new thread`. \
    Once the user approves, run `lucidos planned approve` to flip the marker to gate-satisfying, which unblocks \
    edits and Apply. Lucidos refuses it unless the user tapped an approving \
    option, or typed a plain approval such as `approve`, after you proposed the plan. A tapped \
    fork is an approval too: revise the plan file to that variant, re-commit, then run \
    `lucidos planned approve`. A typed reply that asks, suggests or questions is a request for \
    changes: revise the plan file, re-commit, re-mark with `lucidos planned mark --plan`, and ask \
    again. The new card names only what changed. If \
    this is genuinely a local fix, acknowledge that instead with \
    `lucidos planned mark --simple \"<one-line reason>\"` (no \
    approval needed). A gate-satisfying marker MUST \
    exist before the change can be applied: Claude \
    Code blocks your first source edit until one is set and approved, and Lucidos neither \
    proposes nor applies a change without one. Each Apply consumes it. Writing the plan file \
    itself under `docs/plans/` is never blocked. Keep the plan's load-bearing invariants in view while you edit.";

/// Restart-is-not-rejection note shared by the three recovery system prompts.
///
/// A session resumed after an engine restart replays its own transcript. That
/// may hold a permission denial, an interrupted tool call, or a synthetic
/// "[Request interrupted…]" result. All are artifacts of the restart, not user
/// decisions, and without this note the resumed agent changes course.
///
/// Kept repo-generic, with no Lucidos-only tokens, so it is safe in the
/// external-repo recovery prompt. The companion half is the neutral
/// `RESTART_INTERRUPT_REASON` returned on the permission teardown path.
///
/// Only a recovery spawn carries this prompt. Every other resume, a switch
/// auto-resume or a Continue click among them, hears of the restart through
/// the turn-gap note's restart line (`turn_gap::last_turn_cut_off_by_restart`).
const RESTART_NOT_REJECTION_RULE: &str = "RESTART CONTEXT — NOT A REJECTION: This session was \
    resumed after the engine restarted mid-work. If your recent history shows a permission denial \
    (e.g. \"User denied\"), a tool call that was interrupted or never completed, or a synthetic \
    \"[Request interrupted]\" result, that was caused by the restart — NOT by the user rejecting \
    your work, your plan, or your approach. Do not abandon or rework your approach on account of \
    those signals. Re-confirm where you left off (the git log/diff steps above) and continue the \
    same plan, unless the user has since told you otherwise in a new message.";

/// Teach the *build slot* wrapper to sessions in somebody else's repo.
///
/// Only those. A Lucidos-source session needs none of it: `make lint` and
/// `make test` take a slot themselves. Every line here is paid on every
/// request of every session that carries it. See ADR 0070 and ADR 0210.
const BUILD_SLOT_RULE: &str = "\n\nHEAVY BUILDS TAKE A BUILD SLOT: Sessions run in parallel \
    worktrees, so N simultaneous full builds are N compilers resident on ONE machine, which \
    OOM-kills the host. Prefix a heavy build with `lucidos build-slot -- ` (e.g. `lucidos \
    build-slot -- cargo test --release`). It waits for a free slot, then runs your command niced, \
    on a share of the cores, passing output and exit code through. Do NOT wrap cheap work such as \
    a type-check: that holds a slot for minutes to save seconds. Nothing to release: the slot \
    frees when your command exits, or if the kernel kills it.";

/// Tell the coding agent that background processes do NOT outlive the turn that
/// started them, and name the two waits that work.
///
/// A coding-agent session is a per-turn subprocess. When the turn goes idle the
/// engine tears down the agent and every process it started, whatever group it
/// runs in (`lifecycle::terminate_decision`, `runtime::agent_run_marker`). Left alone the
/// agent trusts its Bash tool's native "runs across turns" contract, which is
/// true for the standalone CLI and false here. Claude Code also dropped its
/// blocking `TaskOutput` wait, so the rule must not name one.
///
/// Three shapes: a foreground call, a polled *background task*, and a watched
/// one. Each foreground call names its 240000 ms ceiling, because the tool
/// default of 120000 ms cuts a build off. The cap keeps every blocking call
/// inside the prompt cache, since a gap past it rewrites the whole context.
/// The lifetime rides the session context ([`prompt_cache_section`]), so this
/// prompt stays byte-identical (ADR 0491). `background-task wait` returns
/// within 230 s, inside the shorter lifetime. A wait
/// with no end in sight takes the watched task and ends the turn: many polls
/// would cost more calls than the one rewrite they save. See
/// `docs/plans/2026-10-13-keep-claude-code-cache-warm-during-waits.md`.
///
/// The example log path is per-worktree (`$(basename "$PWD")`), because
/// concurrent sessions share the temp dir and a fixed name truncates another
/// session's log. It names `${TMPDIR:-/tmp}`, not `/tmp`: on macOS `$TMPDIR` is
/// per-user and owner-only, while `/tmp` is readable by every local user
/// (ADR 0456).
///
/// Claude Code runs with its background tasks switched off (ADR 0358). Its
/// tool results promised a notification that the turn-end teardown never
/// lets arrive, and models believed them over this rule. So `run_in_background`
/// is gone from `Bash` and `Agent`, and passing it fails validation. The rule
/// must never tell the model to pass it, not even as `false`.
///
/// The subagent arm stays because a fan-out is still a wait. Facing "do not
/// end the turn" with no named wait, one session improvised a filler subagent
/// and then a fabricated question. A subagent also blocks the parent mid-turn,
/// where the keep-alive cannot beat it, so long delegated work goes to a child
/// thread instead (ADR 0511).
const BACKGROUND_PROCESS_RULE: &str = "BACKGROUND PROCESSES DON'T SURVIVE A TURN: When your turn \
    ends (you go idle), the Lucidos engine terminates you and every process you started, \
    whatever process group it runs in. A job you detach (`&`, `nohup`, any detached job) is therefore KILLED \
    the instant you end the turn, and nothing re-invokes you when it would have finished. Under \
    Claude Code, Lucidos switches background mode OFF: the `Bash` and `Agent` tools have no \
    `run_in_background` parameter, passing one fails, and a command that outruns its timeout is \
    killed rather than moved to the background. KEEP EVERY BLOCKING CALL INSIDE YOUR PROMPT \
    CACHE: your SESSION CONTEXT says how long it lives, and a longer gap re-bills your whole \
    context. Three shapes. \
    FOREGROUND, for what surely finishes in about 3 minutes: set `timeout: 240000` EXPLICITLY \
    (the 120000 ms DEFAULT cuts a build off at 2 minutes). Overrunning it kills the command. POLLED BACKGROUND TASK, for longer work: `lucidos background-task \
    run --poll --description \"<what it is>\" -- '<cmd>'` runs it in your worktree. Then \
    `lucidos background-task wait <id>` (foreground, `timeout: 240000`) returns within 230 s \
    with the new output; re-issue it until finished. WATCHED BACKGROUND TASK, for an open-ended \
    wait such as a held lock: the same `run` without `--poll` arms an event wait. Name the work in the \
    user's words (\"the full e2e suite\"): the waiting row shows it. On `watched`, say what you \
    wait for and END YOUR TURN: the thread re-opens with the exit status and output tail. On \
    `unwatched` nothing will wake you: stop it and poll instead. REDIRECT \
    a chatty command's output to a log file \
    (`<cmd> > \"${TMPDIR:-/tmp}/$(basename \"$PWD\").log\" 2>&1`) and `tail` it, so a long log \
    never floods your context. SUBAGENTS ARE \
    BACKGROUND WORK TOO: a subagent dies with your process group, so its report must come back \
    inside the turn. Under Claude Code every `Agent` call blocks, so it counts against the cap. \
    Put a fan-out in a single assistant message: its agents run in parallel, and it lasts as long \
    as its slowest agent. Longer delegated work goes to a child thread (`lucidos spawn-thread \
    --relation child`): END YOUR TURN, and the engine keeps a Claude Code cache warm. NEVER \
    improvise a stall instead: a filler subagent, a sleep loop, or a fabricated question (see \
    ASKING USERS).";

/// Send the coding agent's DECISIONS through the structured `AskUserQuestion`
/// tool, which the Lucidos UI renders as clickable buttons. Forbids post-work
/// "does this look good?" confirmations.
///
/// The forbidding half is load-bearing. A held-open question parks the thread in
/// `waiting_for_user_answer`, which stalls hand-off and, in an Apply-based
/// worktree, blocks the Apply button (see [`APPLY_CONFIRMATION_NOTE`]). For a
/// visual change the user cannot judge the result until it has landed, so the
/// two lock each other out.
///
/// The forbidding half over-reaches without an explicit carve-out for **plan
/// approval**. A plan the `implementation-plan` skill committed reads to the
/// model as work it has ALREADY done. So "never CONFIRM finished work" swallows
/// the one approval the plan marker depends on. It is the opposite case in every
/// way that matters: the plan is a proposal about work NOT done, the
/// `cc-plan-gate` hook blocks source edits until the answer arrives, and nothing
/// is appliable yet.
///
/// Keep the carve-out in sync with [`IMPLEMENTATION_PLAN_RULE`],
/// `cc_plan_gate::build_awaiting_approval_json` and the `implementation-plan`
/// skill. All describe the same option FLOOR: `Approve` first, `Request changes`
/// second ONLY when the plan offers no real fork, never a third slot beside one.
/// Stated unconditionally, the pair yields a three-option card whose last button
/// means only "I will type what I want changed". The NEVER AUTHOR AN "OTHER"
/// OPTION paragraph below bans exactly that shape. `Implement in a new thread`
/// replaces the pair when the thread's context is large, but only the CLI names
/// it: this rule also serves external-repo sessions, which have no plan marker.
///
/// The "WHY THE TOOL AND NOT PROSE" paragraph is what makes the two halves
/// cohere instead of reading as a contradiction. Both describe ONE mechanism: a
/// tool call parks the thread in `WaitingForUserAnswer`, the only input to
/// `thread_lifecycle::is_attention_needing`. That predicate lights the
/// Blocked badge, keeps the thread in `DisplaySection::Current` once
/// archived, bubbles up the ancestor chain via `attention_descendant_count`, and
/// fires the "When agent needs me" trigger. Parking is therefore the COST when
/// the work is finished and the POINT when the agent is blocked. A prose
/// question marks nothing, so the thread reads as completed and the user never
/// learns anyone is waiting.
///
/// This shared rule is kept ENVIRONMENT-GENERIC, naming no "Apply" or "Diff",
/// because external-repo prompts interpolate it too and those sessions push and
/// open PRs. The Apply-specific sharpening lives in [`APPLY_CONFIRMATION_NOTE`].
/// Chat-style prompts only: hardening and merge-conflict sessions do not
/// dialogue with the user. Pinned by
/// `chat_style_prompts_nudge_use_of_ask_user_question`.
///
/// The DANGLING ITEM paragraph catches a decision phrased as a plain
/// statement. A report that names an outstanding, parked, or newly unblocked
/// item reads as finished. It hands the user a fork with no way to act. The
/// real case was a session ending on a parked item whose blocker had just
/// cleared. It is carved from the finished-work confirmation ban below,
/// because such an item is a genuine fork, not a "does this look good?".
///
/// The "NEVER AUTHOR AN \"OTHER\" OPTION" paragraph exists to CONTRADICT Claude
/// Code's own built-in tool description, which promises that an "Other" option
/// is provided automatically. CC's TUI provides one; Lucidos does not. The card
/// renders exactly the options passed, and
/// `agent_question::answer_kind_to_hook_value` resolves a `Selected` answer to
/// the option's LABEL. An "Other, I'll type it" button therefore hands that
/// literal phrase back as the user's decision.
///
/// The two real escapes are on every card already, and the prompt row names
/// both to the user: typing in the textarea, by its own placeholder
/// (`PLACEHOLDER_ANSWERING`), and Cancel, by its tooltip
/// (`ANSWER_CANCEL_TOOLTIP`). Mirrored into [`CODEX_ASK_USER_QUESTION_RULE`],
/// which REPLACES this whole constant for Codex, and into the chat-side rule
/// and the `ask_user_question` tool description. Change them together.
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
     work you have NOT done, so route it through this tool with `Approve` and `Request changes` \
     as the options. That pair is a FLOOR, not a fixed shape: the tool needs at least two \
     options, so `Request changes` fills the second slot when the plan offers no real fork. \
     When it DOES offer one (a narrower scope, one layer instead of two, a different \
     approach), make that fork the second option and DROP `Request changes`: a third \
     `Request changes` beside it only means \"I will type what I want changed\", which every \
     card already allows. `Approve` stays first beside a fork. Tapping a fork is still an \
     approval: it approves that variant of the plan, and is not a rejection.\n\n\
     A DANGLING ITEM IS A DECISION IN A STATEMENT'S CLOTHES. Naming something outstanding, \
     unstarted, parked, or newly unblocked hands the user a fork in prose and gives them no way \
     to take it. \"The only thing still pending is X\", \"X is still open\", \"the blocker on X \
     just cleared\" all carry a decision, and none of them is a question, so a report that ends \
     on one reads as finished. This is NOT the finished-work confirmation the rule below \
     forbids: an outstanding item is a real fork, not a \"does this look good?\". Write the \
     sentence, then put the decision on a card in the SAME turn, with the options you would \
     have accepted as a typed reply. The tell is that you can predict what the user will say \
     next.\n\n\
     NEVER AUTHOR AN \"OTHER\" OPTION: do not add an option meaning \"Other\", \"Something \
     else\", \"Let me type it\" or \"I'll write my own answer\". Your tool description says an \
     \"Other\" option is provided automatically. In Lucidos it is NOT: the card renders exactly \
     the options you pass, and tapping one hands you its label back as the user's answer, so an \
     \"Other, I'll type it\" button arrives as their decision and leaves you re-asking. Both \
     escapes are on every card already: the user can type any reply in the prompt textarea, and \
     Cancel dismisses the question so they can steer you somewhere else. An option that carries \
     a decision you can act on is still welcome (\"None of these\", \"Neither, ask me later\", \
     \"Cancel the deploy\"); what is banned is an option whose only meaning is \"I will type it \
     instead\".\n\n\
     QUIET OPTIONS: start a label with `[quiet]` when that pick needs nothing more from you \
     (`[quiet] Not now`). The tag is hidden, and the pick ends your turn: no reply, no more \
     work.\n\n\
     WHY THE TOOL AND NOT PROSE: the tool call is the ONLY thing that tells Lucidos you are \
     waiting. It parks the thread in the waiting-for-answer state, which is what lights the \
     Blocked badge, keeps the thread in the live working set, and can push a \
     notification to the user's phone. A question you type into your final message instead does \
     none of that: the turn ends, the thread reads as FINISHED, and the user gets no signal \
     that you are stuck. So a prose question is not a lighter-touch version of the tool. When \
     you actually need an answer, it is silence. Blocked on the user: ask with the tool. \
     Work finished: do not ask at all, and do not promote a trailing \"does this look good?\" \
     into a tool call. What is always wrong is ending a turn with an \
     unanswered question sitting in your prose.\n\n\
     DO NOT ask a confirmation question about work you've ALREADY done or are wrapping up — \
     \"does this look good?\", \"does this look complete?\", \"did I miss anything?\", \
     \"want me to tweak the color?\". A held-open question parks the thread in the \
     waiting-for-answer state, which stalls hand-off — the user can't take your finished \
     work forward while a question is open. And for a visual or behavioral change the user \
     often cannot judge the result until it's landed and running. When the work is done, \
     finish and hand it off, and let the user review the result. If it needs tuning, \
     they'll tell you in a new message. Ask to DECIDE, never to CONFIRM finished work. Approving \
     a plan you have not implemented yet is a DECISION and is never covered by this paragraph, \
     however finished the plan document itself feels.\n\n\
     NEVER FABRICATE A QUESTION TO STALL: do not call `AskUserQuestion` to keep your turn \
     alive, to hold a background process open, or as a placeholder you expect to be \
     discarded. There is no discard path. Every call is parsed, emitted as a \
     `UserQuestionAsked` event, and rendered as a real card that parks the thread and waits \
     for a human. A dummy question costs the user an interruption they have to clear. \
     Worse, if your own background work then streams steps into the thread it OVERTAKES the \
     card and kills its buttons, so nobody can clear it. When you need to wait, use \
     a wait the background-process rule names.\n\n\
     NEVER parallel-call `AskUserQuestion` alongside other tools — if you're asking a \
     question, stop the assistant message after the `AskUserQuestion` tool_use and do not \
     include any sibling tool_uses (no Bash, no Read, no Agent, no second \
     AskUserQuestion). Lucidos's PreToolUse hook blocks `AskUserQuestion` for up to 24h, \
     but sibling tool_uses in the same message dispatch in parallel and emit progression \
     events while the question is still on-screen, so the user's typed comment can no longer \
     be safely routed as a free-text answer. Wait for the answer, THEN continue.";

/// Apply-specific note, added ONLY by the Apply-based prompt builders
/// (Lucidos-source worktree + recovery, app worktree + recovery). It says an
/// open question blocks the Apply button
/// (`is_blocking` / `available_thread_actions` in `thread_lifecycle.rs`). It is
/// deliberately NOT in the shared rule, because external-repo prompts have no
/// Apply (they push and open PRs) and the Apply-blocking rationale would
/// mislead them into stopping without pushing. Reaches both backends: it is a
/// separate placeholder in the builders, so `append_backend_rules`' Codex swap
/// of `ASK_USER_QUESTION_RULE` leaves it intact.
const APPLY_CONFIRMATION_NOTE: &str = "APPLYING YOUR WORK: An open question BLOCKS the Apply \
     button, and the user can't judge a visual change until they Apply. So when your change is \
     ready, finish the turn and the engine proposes it. Its Apply button sits in this thread. \
     Never tell the user where to apply it (no \"from the Changes panel\").";

/// A session may not settle a question about the shipped product's security,
/// data exposure, or user-visible correctness on its own. When it finds
/// something reaching users that it will not fix, it raises the finding with the
/// question tool. It names the finding in the report. A non-goal in a plan file
/// is not a gate. That gap shipped an unauthenticated app-frame reach, then a
/// clipboard regression. The reasoning and the incident are in the ADR (see ADR
/// 0227, ADR 0231 for the change that produced them).
///
/// Scoped to the six chat-style prompts, the ones that carry
/// `ASK_USER_QUESTION_RULE`. The rule points at the ASKING USERS section, so it
/// can live only where that section is present. `conflict_resolution` has no
/// such section and runs unattended, so it is left out. It is backend-generic
/// on purpose: it names no tool, so the Codex swap of that section leaves it
/// correct. Pinned by
/// `chat_style_prompts_require_raising_user_reaching_findings`.
const RAISE_USER_REACHING_FINDINGS_RULE: &str = "RAISE A USER-REACHING FINDING YOU WON'T FIX, \
    DON'T BURY IT: You may not settle, by yourself, a question about the shipped product's \
    security, data exposure, or user-visible correctness. When you find something you are NOT \
    fixing in this change and it reaches users of the shipped product, raise it with the question \
    tool (named in the ASKING USERS section) BEFORE you finish, and name it in your final report. \
    Recording it as a non-goal, known limitation, follow-up, deferred item, or release gate in a \
    plan file, an ADR, or a commit message does NOT discharge this: a note is not a gate, and your \
    authority stops at the product's users. Judge \"reaches users\" by who could hit it and the \
    harm, NOT by whether it fires on this machine or workspace (a shipped feature is in use by \
    someone). The bar: you are choosing not to fix it, or cannot within your scope, AND it is a \
    silent failure, a security hole, data exposure, or data loss. A finding you DO fix in this \
    change needs no question; one that is out of scope but loud and harmless can stay a note.";

/// Permission allowlist rule — Claude Code ONLY. Lucidos passes
/// `--allowedTools` when spawning CC, which overrides settings.json permission
/// rules, so tool allowlist edits MUST go in the workspace's
/// `.lucidos/cc-allowed-tools` (ADR 0095).
/// None of this applies to Codex (it uses its own sandbox + approval-policy
/// model — approval cards raised by the app-server's `requestApproval`, not
/// `--allowedTools`), so [`append_backend_rules`] appends this only in the
/// `ClaudeCode` arm — mirroring how [`CODEX_CLI_RULE`] is Codex-only.
const PERMISSION_CONFIG_RULE: &str = "\n\n\
    PERMISSION CONFIG: Lucidos passes `--allowedTools` to your Claude Code subprocess. This flag \
    OVERRIDES `~/.claude/settings.json` permission rules, so a tool added to `permissions.allow` \
    there has NO effect and the user keeps seeing the prompt. The prompt card remembers a grant \
    three ways: (1) `Always allow Tool(scope)` (narrow) and (2) `Always allow` (broad) append to \
    `<workspace>/.lucidos/cc-allowed-tools` (one entry per line, `#` comments ignored). It is PER \
    WORKSPACE: a grant binds here and is asked again in another workspace (ADR 0095). The next \
    Claude Code session reads the file at spawn, and the running subprocess keeps its frozen \
    flag. Bare `Edit`/`Write`/`NotebookEdit` cannot be persisted through the broad button: CC \
    settles them itself, so a bare `Edit` line does nothing, and the UI hides that button. \
    (3) `Allow <scope> for this thread` records the pattern in the engine's in-memory per-thread \
    allow set, which works for every tool and path, including CC-protected ones. Lost on engine \
    restart.";

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
    - `lucidos spawn-thread --to <workspace> [--coding-agent claude-code|codex] --message ... \
    --title ...`: spawn a thread (always ask the user first). \
    `--reasoning-effort low|medium|high|xhigh|max` pins its effort. Add `--relation child` \
    only to act on its result.\n\
    - `lucidos await-event --on <EventType> --timeout-secs <n> --reason \"...\"`: subscribe \
    this thread to a Lucidos event, then FINISH your session. It returns immediately and \
    blocks nothing. The engine re-opens this thread with a follow-up message when the event \
    lands, or tells you the deadline passed. Use it instead of a sleep-and-recheck loop \
    whenever you wait on something the engine persists (a change appearing, a trigger \
    firing). NOT for a child you spawned: under `--wake-mode wake`, the default, a failure or a \
    clean report re-opens this thread. `no-wake` files every report. \
    Do NOT poll for it afterwards, and do NOT keep the session alive waiting.\n\
    - `lucidos event-waits list` / `... cancel [--wait-id <id>|--all]`: read or stop this \
    thread's subscriptions. Nothing tells you when one ends, so `list` before claiming you \
    are still watching; `cancel` when they say stop, since saying it does not stop it.";

/// Codex-only slash-command mapping appended by [`append_backend_rules`]
/// alongside [`CODEX_CLI_RULE`]. Lucidos prompts name Claude Code slash
/// commands — the shared [`HARDENING_RULE`] ("run `/harden`"), the
/// merge-conflict prompt's harden step, the engine's auto-harden follow-up
/// (`AUTO_HARDEN_MESSAGE`, "Run /harden now.") and the turn-end gate's harden
/// re-entry — but Codex has no
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
    A DANGLING ITEM IS A DECISION IN DISGUISE: naming something outstanding, parked, or newly \
    unblocked (\"the only thing still pending is X\", \"the blocker on X just cleared\") hands the \
    user a fork with no way to act, and reads as finished because it is not a question. That is a \
    real decision, not the finished-work confirmation above, so put it on a card in the same turn \
    with the options you would have accepted as a reply. \
    Approving a plan BEFORE you implement it is the opposite case: a DECISION you cannot \
    proceed without, about work you have NOT done. Always ask for plan approval through this \
    tool, with `Approve` and `Request changes` as the options, never in plain prose. That pair \
    is a FLOOR: `Approve` comes first, and `Request changes` fills the second slot only when the \
    plan offers no real fork. When it offers one (a narrower scope, one layer instead of \
    two), make that fork the second option and drop `Request changes` rather than carrying it as \
    a third, where it would mean only \"I will type what I want changed\". Tapping a fork is \
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
    QUIET OPTIONS: pass `{label, quiet: true}` when that pick needs nothing more from you \
    (\"Not now\"). Picking it ends your turn: no reply, no more work. \
    WHY THE TOOL AND NOT PROSE: the tool call is the ONLY thing that tells Lucidos you are \
    waiting. It parks the thread in the waiting-for-answer state, which lights the \
    Blocked badge, keeps the thread in the live working set, and can notify the user. A \
    question typed into your final message instead ends the turn, so the thread reads as \
    FINISHED and the user gets no signal that you are stuck. Blocked on the user: ask with this \
    tool. Work finished: don't ask at all, just hand it off. What is always wrong is ending a \
    turn with an unanswered question sitting in your prose. \
    NEVER FABRICATE A QUESTION TO STALL: do not call `ask_user_question` to keep your turn \
    alive, to hold a background process open, or as a placeholder you expect to be discarded. \
    There is no discard path. Every call renders a real card that parks the thread and waits \
    for a human, so a dummy question costs the user an interruption they have to clear. \
    NEVER parallel-call `ask_user_question` alongside other tools — if you're asking a \
    question, stop the assistant message after the `ask_user_question` tool call and do not \
    include any sibling tool calls.";

/// The sentence of [`REASONING_NOT_VISIBLE_RULE`] for a session whose notes
/// reach the user. A macro, so `concat!` can build the rule around it.
macro_rules! notes_reach_user_sentence {
    () => {
        "Before a tool call, your prose may reach them only as a short note of a sentence or \
         two, however much you drafted."
    };
}

/// [`notes_reach_user_sentence`] as a value, which
/// [`with_notes_visibility`] swaps out for a session whose notes are hidden.
const NOTES_REACH_USER_SENTENCE: &str = notes_reach_user_sentence!();

/// What a session whose notes never reach the user reads instead. The agent's
/// own context holds the text it wrote, so it cannot tell by looking.
const NOTES_HIDDEN_SENTENCE: &str = "Text you write in a message that also calls a tool may \
    NEVER reach the user, not even a summary of it, though your own context holds it. A message \
    that calls no tool is shown in full. Put what they must read there, or on the question card, \
    once: never write it before a tool call and again after.";

/// Backend-independent teaching appended to every coding-agent prompt by
/// [`append_backend_rules`], the chokepoint every flavor and both backends ride.
///
/// Neither backend shows the user its reasoning. Claude Code returns thinking
/// as a signature only, or on an always-thinking model as a short progress
/// note when the request asks for one (`runtime/vertex_relay.rs`,
/// `runtime/claude_code_provider.rs`). Codex streams a lossy summary
/// (`CODEX_REASONING_SUMMARY` in `runtime/codex.rs`). An agent that drafts
/// user-facing content there and then points at it points at nothing. Two
/// real cards: "Caption copy: do the six lines above work?", and later "the
/// card copy above". The API never returns the full text, so this is guidance,
/// backed by `question_card_gate` refusing a card that says "above".
const REASONING_NOT_VISIBLE_RULE: &str = concat!(
    "\n\n\
    YOUR REASONING IS NOT SHOWN TO THE USER: The user sees only your visible assistant messages \
    and your tool calls, never your reasoning. So anything they must see or act on (draft copy \
    to approve, the options behind a question, a snippet to review, what you found) MUST go in a \
    visible assistant message, or in a tool field the UI renders, such as a question tool's \
    `question` / `options`. Never reference content as if they saw it (\"the six lines above\") \
    unless you put it in a visible message this turn. ",
    notes_reach_user_sentence!(),
    " So content they \
    must read in full goes on the question card itself, in the option descriptions or the \
    question, which renders markdown: short paragraphs or a list, the question last."
);

/// `prompt` as a session with these notes reads it. A session whose notes
/// reach the user keeps the prompt as built. `append_backend_rules` put the
/// sentence there, so a hidden session reads the hard rule instead. The
/// verdict is read once per spawn, so the prompt holds still across its turns.
pub(crate) fn with_notes_visibility(
    prompt: String,
    notes: crate::runtime::claude_code_provider::NotesVisibility,
) -> String {
    use crate::runtime::claude_code_provider::NotesVisibility;
    match notes {
        NotesVisibility::Shown => prompt,
        NotesVisibility::Hidden => prompt.replace(NOTES_REACH_USER_SENTENCE, NOTES_HIDDEN_SENTENCE),
    }
}

/// Backend-independent: any session can write an HTML file for the user. The
/// type scale lived only in the `lucidos-cli` skill, and a session that never
/// loaded it wrote a 16px scale with `4rem` headings (ADR 0319).
const HTML_ARTIFACT_TYPE_SCALE_RULE: &str = "\n\n\
    HTML ARTIFACT TYPE SCALE: Size an HTML file under `artifacts/` like the chat: body \
    `0.75rem`, labels `0.8125rem`; `1rem` is a heading. Use `rem` and never size the root, \
    since the preview applies the UI scale. Full scale: `lucidos knowhow read \
    system-knowhow/best-practices`, § Standalone HTML.";

/// Sibling of [`REASONING_NOT_VISIBLE_RULE`], riding the same
/// [`append_backend_rules`] chokepoint, and the same shape of mistake: the
/// agent believes the user can see what only it can see.
///
/// A session rendered a docs page to a PNG, called `Read` on it, and told the
/// user "I did send an image". Reading an image is an INPUT: Claude Code hands
/// the picture to the agent's own context and nothing else. So the agent could
/// genuinely see the page while the user saw an empty step.
///
/// The rule names the alternative rather than only prohibiting the mistake,
/// because not knowing the alternative is what the transcript shows. An
/// artifact plus markdown image syntax renders inline, since `renderMarkdown`
/// rewrites a workspace-relative source onto the `/data` mount. Full trace in
/// `docs/plans/2026-08-26-cc-tool-results-and-showing-the-user-an-image.md`.
///
/// A later session knew the alternative and still stopped halfway. It saved the
/// picture, wrote "I've drawn out the options", and never pasted the image
/// line. So the rule names saving as showing nothing too. `lucidos data write`
/// says the same when it saves a picture.
///
/// The card comes first, with its reason, because a reply written just before
/// a card arrives as a short summary without the picture. The
/// `question_card_gate` refuses such a card once:
/// `docs/plans/2026-09-25-a-card-after-a-picture-nobody-saw.md`.
///
/// The visual-choice clause counters Claude Code's own tool text, which calls
/// `preview` the place for mockups and shows it as monospace text.
///
/// The tell is deliberately the STEP'S SUBJECT, never the label
/// `claude_code_parse::describe_content_block` renders. A rule naming a
/// rendering detail goes stale the next time one moves. It then teaches the
/// agent a test that no longer distinguishes anything.
///
/// Mirrored for the chat agent by the FILE REFERENCES section of
/// `chat::process::system_prompt`, which made the same mistake for the same
/// reason: it taught that a path becomes a link and stopped there. It is
/// phrased differently because that agent writes `data/` directly and needs no
/// `lucidos data write`. Its question-card clause names an option's
/// description, since that agent's question tool has no `preview`
/// (`docs/plans/2026-09-24-pictures-on-question-cards.md`). Change both together.
const SHOWING_AN_IMAGE_RULE: &str = "\n\n\
    READING OR SAVING AN IMAGE DOES NOT SHOW IT TO THE USER: `Read` on a PNG is an INPUT. It \
    puts the picture in YOUR context and sends the user nothing: their step records that you \
    read a file, never the picture. `lucidos data write artifacts/x.png` only stores it, and the user still sees nothing. It appears ONLY where you paste the line \
    it prints, `![what it shows](artifacts/x.png)`. Before a card, put it ON the question card \
    (its text or an option's `preview` if your tool has one): words before a tool call arrive \
    as a short summary that drops it. Else, in the reply ending your turn. Same for a render or \
    chart. SHOW A VISUAL CHOICE AS PICTURES. For mockups or options that differ in colour, \
    layout or type, render real ones. A headless-browser screenshot of the actual CSS works. \
    ASCII art shows no colour. Give each option its own picture of only that option. Put it in \
    that option's `preview` if your tool has one, else label it in the question. Never repeat \
    one combined sheet on every option. The `!` on an `app:` link draws a live widget the same \
    way (see Widget embeds).";

/// How a coding agent links a line it cites, so a click opens the file
/// preview modal there. It names the session's own repository id, which the
/// agent cannot look up from inside its worktree, and its branch: the agent
/// mostly cites its own work, which the clone's `HEAD` does not have yet.
/// Empty for an unregistered Lucidos checkout, whose files have no link form.
///
/// Mirrored for the chat agent by the FILE REFERENCES section of
/// `chat::process::system_prompt`. Change both together.
pub(super) fn file_link_rule(repo_id: Option<&str>, app_id: Option<&str>, branch: &str) -> String {
    let example = match (app_id, repo_id) {
        (Some(app), _) => format!("[app.js:5-9](apps/{app}/app.js#L5-L9)"),
        (None, Some(repo)) => {
            format!("[main.rs:5-9](repo:{repo}:file#{branch}:src/main.rs#L5-L9)")
        }
        (None, None) => return String::new(),
    };
    format!(
        "\n\nLINK A FILE YOU CITE, or it is not clickable: `{example}`, with the path from the \
         repository root. The `#L` suffix opens it at those lines; `#L5` is one line."
    )
}

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
    in git commands, tool arguments, and CLI calls. A markdown link TARGET is not prose \
    either, so keep pasting `[title](thread:<ws>/<uuid>)` exactly as `lucidos spawn-thread` \
    prints it: without the link the user cannot open the thread you started. The only other exception is a raw value the user asked for, or one they have to \
    paste somewhere.";

/// One-line mirror of the chat agent's
/// `chat::process::system_prompt::NO_IMPERSONATION_RULE`, which carries the
/// full reasoning and the incident it comes from. A coding agent
/// holds the same Bash capability and the same `lucidos` CLI, so it can reach
/// the engine's API by hand exactly as the chat agent did; it is short here
/// because a coding-agent session's own scope makes the temptation rarer.
/// Change both together.
const NO_IMPERSONATION_RULE: &str = "\n\n\
    NEVER POST TO THE LUCIDOS ENGINE API AS THE USER: Do not curl (or otherwise hand-roll HTTP \
    to) the engine's own `/api/v1` surface to do something a tool or the `lucidos` CLI refuses \
    you, and never record a message as though the user typed it. When you are blocked, say so \
    and offer what you CAN do. A refusal reported honestly is a good turn; a refusal worked \
    around is a broken one, however well it appears to succeed.";

/// Backend-independent, riding [`append_backend_rules`] like the rules above.
///
/// The permission card leads with the agent's own line about a command: Claude
/// Code's Bash `description`, or the justification Codex sends as `reason`
/// (`renderCommandAsk` in `PermissionCard.tsx`). Claude Code's own tool text
/// asks only for what a command does. A user deciding whether to allow it also
/// needs why, so the rule asks for both.
const PERMISSION_ASK_RULE: &str = "\n\n\
    SAY WHAT A COMMAND DOES AND WHY: A permission card shows a command's description as its \
    question (Codex: your escalation justification). Write it for the user: what it does and \
    why, like \"Read the triggers doc, to see how triggers install\", not \"Run sed\".";

/// Backend-independent, riding [`append_backend_rules`] like the rules above.
///
/// Every turn makes a *read decision* (ADR 0417). A coding agent's ready
/// change already lists its thread under Review. A session that ends with an
/// answer instead, and no change, says yes to get there (ADR 0409). The
/// turn-end gate forces a decision the session skipped. The chat agent learns
/// the same from the `request_read` tool's own description.
const READ_DECISION_RULE: &str = "\n\n\
    READ DECISION: End every turn that asks no question with `lucidos request-read yes|no`, \
    in its own message, then answer once. Yes for an answer and no change (an investigation, a \
    review), or one read later. Else no.";

/// Backend-independent, riding [`append_backend_rules`] like the rules above.
///
/// Tool output is most of a coding session's input cost, and whole-file reads
/// are its biggest share. The rule teaches the cheap shape (locate, then read a
/// window) in every repository, which a path-scoped rule file cannot reach. It
/// adds no output limit, since a lower limit only moves the same reads into
/// more calls (`docs/plans/2026-10-20-cut-tool-output-and-rule-load-cost.md`).
const READ_NARROWLY_RULE: &str = "\n\n\
    READ NARROWLY: Locate before you read. Find the code with Grep or `rg -n` first, adding \
    `-l` or `-c` when only the location matters. Then read about 100 lines around the hit. \
    Read a whole file only when you need all of it. Do not re-read lines \
    already in your context unless the file changed or you are about to edit them.";

/// How a session continues in a *successor thread*. The procedure lives in
/// `lucidos successor --help`. Advice from any other source to `/clear` or to
/// write a handoff file is read the same way, without naming that source.
const SUCCESSOR_THREAD_RULE: &str = "\n\n\
    SUCCESSOR THREADS: When the user says \"handoff this session\", commit your work, write a \
    handoff narrative and run `lucidos successor spawn --narrative <file>` (see its --help). In a \
    Lucidos thread, advice to run `/clear` or write a handoff file means the same. A HANDOFF \
    CHECKPOINT is a `lucidos successor offer --at <name>` step in a repo flow: run it and follow \
    it. Setup: `lucidos knowhow read system-knowhow/coding-agent-flows`.";

/// Told only to a sub-thread's session whose parent declared no wake mode. The
/// chat agent learns the same from the `wake_parent` tool's own description.
const WAKE_DECISION_RULE: &str = "\n\n\
    WAKE DECISION: You are a sub-thread, and your turn's final message reports to your parent \
    thread. Before that message, run `lucidos wake-parent yes|no`. Yes when your parent must \
    act on it now: it waits for this result to take a next step, combine results, retry or \
    decide. No when it only informs: the report is filed, and your parent reads it on its next \
    turn. A failure always wakes it. Unsure, skip it: Lucidos decides.";

/// The [`WAKE_DECISION_RULE`] section for a session on `thread_id`, or empty
/// when the thread has no parent, or its spawn declared a wake mode.
pub(super) async fn wake_decision_section(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> &'static str {
    if crate::engine::turn_end::wake_decision::WakeDuty::owes_a_decision(pool, thread_id).await {
        WAKE_DECISION_RULE
    } else {
        ""
    }
}

/// Told only to an *integrated sub-thread*'s session, naming the parent it
/// folds into. The parent learns its half from the child's report and from the
/// `lucidos-cli` skill.
pub(super) async fn integrated_section(pool: &sqlx::PgPool, thread_id: uuid::Uuid) -> String {
    let parent_title: Option<String> = sqlx::query_scalar(
        "SELECT p.title FROM thread_summaries t \
         JOIN thread_summaries p ON p.thread_id = t.parent_thread_id \
         WHERE t.thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log!("[AgentSession] parent title for {}: {}", thread_id, e);
        None
    })
    .flatten();
    integrated_rule(parent_title.as_deref())
}

/// The integrated sub-thread section, for a parent titled `parent_title`.
fn integrated_rule(parent_title: Option<&str>) -> String {
    let parent = match parent_title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => format!("your parent thread \"{title}\""),
        None => "your parent thread".to_string(),
    };
    format!(
        "\n\nINTEGRATED SUB-THREAD: You are integrated into {parent}. Your branch folds into \
         its branch, so Lucidos proposes nothing for yours: there is no change and no Apply. \
         Run `/harden` before you finish, as always. It runs the fast tier: review, lint and \
         the frontend checks, with no engine suite, and it skips merging main. Your parent's \
         own /harden runs every suite once over the merged work. Commit everything, and do not \
         merge main yourself. End with `lucidos wake-parent yes`: your report names your \
         branch, and your parent's agent merges it."
    )
}

/// Told only to a session whose turn the owner did not open (ADR 0387).
/// Without it the agent learns from the 403, after an ordinary card's answer
/// that grants nothing.
const NO_STANDING_INSTRUCTION_NOTE: &str = "\n\n\
    THE OWNER DID NOT OPEN THIS TURN: another thread or an event did. So Lucidos refuses, with a \
    403, an act on a thread outside your own subtree, such as an Apply, archive or cancel. \
    Spawning a thread, top-level or child, needs no authority. An ordinary question card cannot \
    authorize such an act, even when the owner picks the option naming it. When the decision \
    you would ask about is one, ask for an owner approval INSTEAD of an ordinary card. Run \
    `lucidos ask-owner-approval <verb> --thread <uuid> --reason \"<why>\"` \
    (`lucidos ask-owner-approval --help` lists the verbs). Then ask your question tool with the \
    id it prints as the only question. Lucidos shows its own owner approval card, with Allow \
    once, in its place. A 409 from it means you hold the authority after all, for example \
    because the owner has written to you since. Then go ahead as usual.";

/// The [`NO_STANDING_INSTRUCTION_NOTE`] section for a session on `thread_id`,
/// or empty when its current turn carries the owner's standing instruction.
/// Reads the gate's own definition, so the note and the 403 cannot disagree.
pub(super) async fn standing_instruction_section(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> &'static str {
    if crate::api::standing_instruction::carries_standing_instruction(pool, Some(thread_id), None)
        .await
    {
        ""
    } else {
        NO_STANDING_INSTRUCTION_NOTE
    }
}

/// Opens the [`SessionContext`] block. The base prompts point at it by name.
const SESSION_CONTEXT_HEADER: &str = "SESSION CONTEXT: Lucidos sends this once, at the start of \
    your session. It holds what differs from one session to the next. Your system prompt holds \
    the rest.";

/// Closes the [`SessionContext`] block.
const SESSION_CONTEXT_END: &str = "---\nEnd of session context.";

/// The *session context block* (ADR 0491): what one session needs and the next
/// does not. It leads the first message of a session that starts fresh, so the
/// appended system prompt stays byte-identical across sessions.
#[derive(Debug, Default)]
pub(super) struct SessionContext {
    sections: Vec<String>,
    images: Vec<crate::api::ChatImage>,
}

impl SessionContext {
    /// Add one section. A blank one is skipped, so a conditional section that
    /// came out empty leaves no gap.
    pub(super) fn push(&mut self, section: &str) {
        let section = section.trim();
        if !section.is_empty() {
            self.sections.push(section.to_string());
        }
    }

    /// Images a section refers to, which the first message carries ahead of
    /// its own.
    pub(super) fn attach_images(&mut self, images: Vec<crate::api::ChatImage>) {
        self.images.extend(images);
    }

    /// The block as the first message carries it.
    pub(super) fn render(self) -> crate::runtime::SessionContextBlock {
        crate::runtime::SessionContextBlock {
            text: format!(
                "{SESSION_CONTEXT_HEADER}\n\n{}\n\n{SESSION_CONTEXT_END}",
                self.sections.join("\n\n")
            ),
            images: self.images,
        }
    }
}

/// The prompt cache lifetime the [`BACKGROUND_PROCESS_RULE`] cap points at
/// (ADR 0511). An unknown TTL states 5 minutes, as the keep-alive beats it
/// (ADR 0479). Codex states 5 minutes too: its cache is OpenAI's.
pub(super) fn prompt_cache_section(
    observed: Option<CacheTtl>,
    coding_agent: crate::runtime::CodingAgent,
) -> String {
    let ttl = match coding_agent {
        crate::runtime::CodingAgent::ClaudeCode => observed.unwrap_or(CacheTtl::FiveMinutes),
        crate::runtime::CodingAgent::Codex => CacheTtl::FiveMinutes,
    };
    format!(
        "PROMPT CACHE: Your prompt cache lives {}. Keep every blocking call, subagents \
         included, under {}.",
        ttl.lifetime_words(),
        ttl.blocking_cap_words()
    )
}

/// Opens the branch section. The e2e fake Claude Code reads the branch here.
const BRANCH_SECTION_LEAD: &str = "BRANCH: Your worktree is on branch";

fn branch_section(branch: &str, base_ref: Option<&str>) -> String {
    match base_ref {
        Some(base) => format!("{BRANCH_SECTION_LEAD} `{branch}`, based on {base}."),
        None => format!("{BRANCH_SECTION_LEAD} `{branch}`."),
    }
}

/// What a spawn's two prompts are built from (ADR 0491). Only `flavor`,
/// `coding_agent` and `workspace_sections` reach the appended system prompt.
pub(super) struct SpawnPromptInputs<'a> {
    /// The flavor's prompt, which names no session value.
    pub(super) flavor: String,
    pub(super) coding_agent: crate::runtime::CodingAgent,
    /// Sections that vary by workspace or build only, appended in order.
    pub(super) workspace_sections: &'a [&'a str],
    pub(super) branch: &'a str,
    /// An external repo's base ref.
    pub(super) base_ref: Option<&'a str>,
    pub(super) repo_id: Option<&'a str>,
    pub(super) app_id: Option<&'a str>,
    pub(super) wake_section: &'a str,
    /// The repo's observed prompt cache TTL.
    pub(super) cache_ttl: Option<CacheTtl>,
}

/// A spawn's appended system prompt, and the start of its session context.
pub(super) fn spawn_prompts(inputs: SpawnPromptInputs<'_>) -> (String, SessionContext) {
    let mut system_prompt = append_backend_rules(inputs.flavor, inputs.coding_agent);
    for section in inputs.workspace_sections {
        system_prompt.push_str(section);
    }
    let mut context = SessionContext::default();
    context.push(&branch_section(inputs.branch, inputs.base_ref));
    context.push(&file_link_rule(
        inputs.repo_id,
        inputs.app_id,
        inputs.branch,
    ));
    context.push(inputs.wake_section);
    context.push(&prompt_cache_section(inputs.cache_ttl, inputs.coding_agent));
    (system_prompt, context)
}

/// Append backend-specific rules — plus the backend-independent
/// [`REASONING_NOT_VISIBLE_RULE`], which rides every prompt here — to a finished
/// system prompt — the single
/// point where the two coding-agent backends diverge. Claude Code prompts gain
/// [`PERMISSION_CONFIG_RULE`] (the `--allowedTools` / `cc-allowed-tools`
/// mechanics are CC-only); Codex prompts replace [`ASK_USER_QUESTION_RULE`]
/// with [`CODEX_ASK_USER_QUESTION_RULE`] and append [`CODEX_CLI_RULE`]. Each
/// backend gets ONLY its own section: the CC permission-config rule would be
/// misleading noise on a Codex session, which surfaces permissions through its
/// sandbox + approval-policy model (approval cards raised by the app-server),
/// not `--allowedTools`.
/// Called from [`spawn_prompts`] with the backend `run_direct_agent` already
/// resolved. Do NOT re-query `thread_summaries` here.
pub(super) fn append_backend_rules(
    prompt: String,
    coding_agent: crate::runtime::CodingAgent,
) -> String {
    // Backend-INDEPENDENT: neither backend shows the user the model's reasoning
    // or an image it read, and both talk about work they track by sha. So every
    // prompt flavor gets these rules here (the shared chokepoint) before the
    // backend-specific teaching below.
    let prompt = format!(
        "{prompt}{REASONING_NOT_VISIBLE_RULE}{SHOWING_AN_IMAGE_RULE}\
         {HTML_ARTIFACT_TYPE_SCALE_RULE}{NAMES_NOT_IDS_RULE}{NO_IMPERSONATION_RULE}\
         {PERMISSION_ASK_RULE}{READ_DECISION_RULE}{READ_NARROWLY_RULE}{SUCCESSOR_THREAD_RULE}"
    );
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
pub(super) fn worktree_system_prompt(workspace_name: &str) -> String {
    format!(
        "{preamble}\n\n\
         WORKTREE CONTEXT: You are running in an isolated git worktree on the branch your \
         SESSION CONTEXT names. Your working directory is a complete copy of the Lucidos \
         repository. Your changes stay isolated until the user applies them.\n\n\
         ISOLATION RULES: Your worktree is your entire world. ALL file edits, builds, and \
         test runs MUST happen inside your worktree directory (your cwd). Never `cd` to or \
         modify files in the main repository. Never reference absolute paths to the main repo. \
         The scripts (`scripts/e2e.sh`, etc.) resolve paths from where they live and `cargo` \
         from the `Cargo.toml` in your cwd, so running them from your worktree uses your \
         worktree's code. \
         E2E: just run `./scripts/e2e.sh` (full API + browser) or `./scripts/e2e-api.sh` / \
         `./scripts/e2e-browser.sh` for one suite. They run on GitHub by default; an \
         uncommitted tree or `--local` runs here, on its own session-scoped engine for the \
         disposable `e2e-test` workspace. There is NO separate start step. \
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
         {no_memory_files}\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         {raise_findings}\n\n\
         {apply_confirmation}\n\n\
         {background_process}\n\n\
         SESSION SUMMARY: After hardening completes, output a structured summary of what \
         was implemented in this session. List each change with its status (committed, applied, \
         pending). Include file names and brief descriptions. This is the last thing you output \
         before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}",
        preamble = workspace_preamble(workspace_name),
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        implementation_plan = IMPLEMENTATION_PLAN_RULE,
        apply_restart = APPLY_RESTART_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        no_memory_files = NO_MEMORY_FILES_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(true),
    )
}

/// Build the system prompt for external repository worktree sessions.
pub(super) fn external_repo_system_prompt(repo_name: &str) -> String {
    format!(
        "REPOSITORY CONTEXT: You are working in an isolated git worktree of the \"{repo_name}\" \
         repository, on the branch your SESSION CONTEXT names.\n\n\
         You have full git access — push and open PRs as needed; the user's git credentials and \
         CLI tools (gh, etc.) are available.\n\n\
         STAY ON THIS WORKTREE'S BRANCH: Do ALL of your committed work on the branch this \
         worktree is checked out on. Lucidos tracks THIS branch for the thread's Diff \
         view and for resuming you, and the branch you push must be the same one. If the repo's \
         workflow wants a differently-named branch (e.g. a ticket branch like `JIRA-1234-...`), \
         RENAME this branch in place with `git branch -m <new-name>` and keep \
         working on it — do NOT `git checkout -b` a separate sibling branch, commit there, and \
         leave this worktree behind. Stranding later commits (e.g. a pre-PR cleanup pass) on a \
         branch this worktree is not on makes the Diff show stale, pre-cleanup work that no longer \
         matches your PR. Whatever branch you finish on MUST be the one this worktree is checked \
         out on — run `git branch --show-current` before you finish to confirm.\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check for \
         uncommitted changes. Commit or discard anything unintentional.\n\n\
         {commit_cadence}\n\n\
         {ask_user_question}\n\n\
         {raise_findings}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or stop, \
         simply say goodbye and finish your response — the Lucidos engine manages your lifecycle. \
         Running `exit` in bash can crash the host application.{process_safety}{build_slot}",
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(false),
        build_slot = BUILD_SLOT_RULE,
    )
}

/// Build the system prompt for recovered orphaned worktree sessions in external repos.
pub(super) fn external_repo_recovery_system_prompt(repo_name: &str) -> String {
    format!(
        "RECOVERED SESSION: You are running in a worktree on your own branch of the \"{repo}\" \
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
         {raise_findings}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}{build_slot}",
        repo = repo_name,
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        background_process = BACKGROUND_PROCESS_RULE,
        process_safety = process_safety_rule(false),
        build_slot = BUILD_SLOT_RULE,
    )
}

/// Build the system prompt for recovered Lucidos-source worktree sessions.
/// The LLM already has the original thread's message history, so this prompt
/// just explains the restart context and tells it to review and continue.
/// Three sibling recovery builders exist for the other worktree flavors:
/// `external_repo_recovery_system_prompt`, `app_worktree_recovery_system_prompt`.
pub(super) fn recovery_system_prompt(workspace_name: &str) -> String {
    format!(
        "{preamble}\n\n\
         RECOVERED SESSION: The Lucidos engine restarted while you were working on your \
         branch. Your previous session was interrupted but the worktree is intact.\n\n\
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
         {no_memory_files}\n\n\
         COMMANDS: Never use /cpa — it is for the main working tree only. \
         Just commit directly with `git add <file>` + `git commit -m \"message\"`. \
         The engine pushes to remote after the user clicks Apply (which is what merges your \
         branch into main).\n\n\
         {no_pull_requests}\n\n\
         {hardening}\n\n\
         {ask_user_question}\n\n\
         {raise_findings}\n\n\
         {apply_confirmation}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}",
        preamble = workspace_preamble(workspace_name),
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        no_pull_requests = NO_PULL_REQUESTS_RULE,
        implementation_plan = IMPLEMENTATION_PLAN_RULE,
        apply_restart = APPLY_RESTART_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        no_memory_files = NO_MEMORY_FILES_RULE,
        hardening = HARDENING_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
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
    workspace_name: &str,
    app_id: &str,
    app_manifest_json: &str,
) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         You are editing the `{app_id}` app in this workspace.\n\n\
         APP WORKTREE CONTEXT: You are running in an isolated git worktree of the user's \
         Lucidos *workspace* git (not the Lucidos source), narrowed by sparse-checkout to \
         one app folder, on the branch your SESSION CONTEXT names. \
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
         APPLY: When you finish, your commits become a pending *change* on this thread. \
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
         {raise_findings}\n\n\
         {apply_confirmation}\n\n\
         {background_process}\n\n\
         SESSION SUMMARY: Output a structured summary of what was implemented in this \
         session. List each change with a brief description. This is the last thing you \
         output before finishing.\n\n\
         CRITICAL: Never run `exit` as a bash command. If the user asks you to exit or \
         stop, simply say goodbye and finish your response — the Lucidos engine manages \
         your lifecycle. Running `exit` in bash can crash the host application.{process_safety}",
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
        background_process = BACKGROUND_PROCESS_RULE,
        app_knowhow = APP_KNOWHOW_RULE,
        process_safety = process_safety_rule(false),
    )
}

/// Build the system prompt for recovered orphaned app worktree sessions.
pub(super) fn app_worktree_recovery_system_prompt(workspace_name: &str, app_id: &str) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         You are editing the `{app_id}` app in this workspace.\n\n\
         RECOVERED SESSION: The Lucidos engine restarted while you were working on your \
         branch in an isolated app worktree (sparse-checkout of the workspace git \
         narrowed to `data/apps/{app_id}/`). Your previous session was interrupted but \
         the worktree is intact.\n\n\
         Review the message history above to understand what you were working on, then:\n\
         1. Run `git log --oneline main..HEAD` to see what commits were made\n\
         2. Run `git diff` to check for uncommitted changes\n\
         3. If the work looks complete, clean up and finish\n\
         4. If incomplete, continue where you left off\n\n\
         {restart_not_rejection}\n\n\
         When you finish, your commits become a pending *change* on this thread. Apply \
         ff-merges your branch into the workspace git's `main`. No engine restart; no \
         `/harden` (apps own their hardening). Apply emits `AppUiRefreshRequested` if any \
         iframe-bundled file changed.\n\n\
         {app_knowhow}\n\n\
         CLEAN UP BEFORE FINISHING: Before ending your session, run `git diff` to check \
         for uncommitted changes. Discard unintentional changes with `git checkout -- <file>`.\n\n\
         {commit_cadence}\n\n\
         COMMANDS: Never use /cpa. Just commit with `git add` + `git commit -m \"…\"`.\n\n\
         {ask_user_question}\n\n\
         {raise_findings}\n\n\
         {apply_confirmation}\n\n\
         {background_process}\n\n\
         CRITICAL: Never run `exit` as a bash command.{process_safety}",
        restart_not_rejection = RESTART_NOT_REJECTION_RULE,
        commit_cadence = COMMIT_CADENCE_RULE,
        ask_user_question = ASK_USER_QUESTION_RULE,
        raise_findings = RAISE_USER_REACHING_FINDINGS_RULE,
        apply_confirmation = APPLY_CONFIRMATION_NOTE,
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
    // `apply_now`). Lucidos-source threads run `/harden`, which also selects
    // and runs the test suites for what changed.
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

/// The system prompt of a *lane thread*: the coding-agent thread that resolves
/// a lane batch on its integration branch (ADR 0463). Its first prompt
/// ([`build_lane_thread_prompt`]) names the members.
pub(crate) fn lane_thread_system_prompt() -> &'static str {
    "LANE THREAD: You resolve a lane batch. These are changes that would not merge \
     cleanly onto main, so they land together. You work in the lane worktree, on the \
     batch's integration branch. Nothing outside it is yours to change.\n\n\
     RULES:\n\
     - Merge each member exactly as your prompt lists it, one `git merge --no-ff` per \
       member. That merge commit is the change's landing merge, so Revert later undoes \
       exactly one change.\n\
     - Resolve a conflict inside the merge that raised it. Read both sides, keep both \
       intents working, `git add` each file, then `git commit --no-edit`.\n\
     - Never rebase, never amend or squash a landing merge, and never reset the branch.\n\
     - Never touch another worktree, least of all a source thread's.\n\
     - When a resolution needs a judgment, ask the user with the question tool before \
       you choose. Two changes that each add a competing mechanism is such a case.\n\
     - After the last merge, run `/harden` once on the merged result.\n\n\
     When everything is merged and hardened, end your turn with one short message: \
     each conflicted file, and one sentence on how you resolved it. The engine lands \
     the batch when your turn ends cleanly. A Stop or a failure fails the whole batch, \
     and every member stays pending.\n\n\
     COMMANDS: Never use /cpa. Commit with `git add` and `git commit` directly.\n\n\
     CRITICAL: Never run `exit` as a bash command."
}

/// What a lane thread's prompt says about one member of its batch.
pub(crate) struct LaneMemberBrief<'a> {
    /// The member's thread title. It names the landing merge.
    pub(crate) title: &'a str,
    /// What the change does, from its description.
    pub(crate) summary: &'a str,
    pub(crate) branch: &'a str,
    /// The exact commit the lane lands for it.
    pub(crate) snapshot_sha: &'a str,
}

/// Longest member summary a lane prompt quotes. The thread title already says
/// what the change is, so the summary only adds intent.
const LANE_SUMMARY_CHARS: usize = 400;

/// The first prompt of a lane thread: one landing merge per member, in lane
/// order, then one `/harden`.
pub(crate) fn build_lane_thread_prompt(members: &[LaneMemberBrief<'_>]) -> String {
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
pub(crate) fn build_lane_catch_up_prompt(main_sha: &str, commits: &[String]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Code marks a quiet option with the label prefix the parser
    /// strips, so the rule must name that exact prefix.
    #[test]
    fn the_claude_code_rule_names_the_quiet_label_prefix() {
        let prefix = crate::engine::agent_session::parsing::QUIET_LABEL_PREFIX;
        assert!(ASK_USER_QUESTION_RULE.contains(&format!("`{prefix}`")));
        assert!(CODEX_ASK_USER_QUESTION_RULE.contains("quiet: true"));
    }

    /// The read decision runs on every turn, before the answer. Claude Code
    /// allows that command up front, so no permission card stalls the turn.
    #[test]
    fn the_read_decision_command_is_allowed_without_a_card() {
        assert!(READ_DECISION_RULE.contains("`lucidos request-read yes|no`"));
        assert!(crate::engine::claude_code::ENGINE_ALLOWED_TOOLS
            .contains(&"Bash(lucidos request-read:*)"));
    }

    /// The rule exempts exactly what Apply exempts, by naming the predicate
    /// Apply reads. The binding below fails to compile once it is renamed.
    #[test]
    fn the_hardening_rule_exempts_by_the_engine_plan_file_predicate() {
        let _: fn(&str) -> bool = crate::engine::git_ops::is_plan_file;
        assert!(HARDENING_RULE.contains("(`git_ops::is_plan_file`)"));
        assert!(HARDENING_RULE.contains("skip `/harden` there."));
    }

    /// An integrated sub-thread learns whom it folds into, that it runs the
    /// fast tier and proposes nothing, and that its parent must hear now.
    #[test]
    fn the_integrated_section_names_the_parent_and_the_fast_tier() {
        let rule = integrated_rule(Some("Fix the drawer"));
        assert!(rule.contains("integrated into your parent thread \"Fix the drawer\""));
        assert!(rule.contains("`/harden`"), "{rule}");
        assert!(rule.contains("fast tier"), "{rule}");
        assert!(rule.contains("proposes nothing"), "{rule}");
        assert!(rule.contains("`lucidos wake-parent yes`"), "{rule}");
        assert!(integrated_rule(None).contains("integrated into your parent thread."));
        assert!(integrated_rule(Some("  ")).contains("integrated into your parent thread."));
    }

    /// Scratch files go to the per-user temp dir. `/tmp` is readable by every
    /// local user, and on macOS `$TMPDIR` is not (ADR 0456).
    #[test]
    fn scratch_paths_name_the_per_user_temp_dir() {
        assert!(BACKGROUND_PROCESS_RULE.contains("${TMPDIR:-/tmp}/"));
        for rule in [BACKGROUND_PROCESS_RULE, SHOWING_AN_IMAGE_RULE] {
            let rest = rule.replace("${TMPDIR:-/tmp}/", "");
            assert!(!rest.contains("/tmp/"), "a bare /tmp scratch path: {rule}");
        }
    }

    /// A reply's file link opens the file preview modal only when the UI can
    /// resolve it. So the rule names the session's own locator, at its branch.
    #[test]
    fn file_link_rule_names_the_sessions_own_locator() {
        let repo = file_link_rule(Some("3f9c1b2e"), None, "feature/x");
        assert!(
            repo.contains("(repo:3f9c1b2e:file#feature/x:src/main.rs#L5-L9)"),
            "{repo}"
        );
        let app = file_link_rule(None, Some("habit-tracker"), "feature/x");
        assert!(app.contains("(apps/habit-tracker/app.js#L5-L9)"), "{app}");
        assert_eq!(file_link_rule(None, None, "feature/x"), "");
    }

    /// ADR 0491: two sessions of one workspace and flavor present the same
    /// appended prompt bytes, so they share its cache entry. What sets them
    /// apart rides each one's session context instead.
    #[test]
    fn two_sessions_in_one_workspace_share_the_appended_prompt() {
        let workspace_sections = ["\n\nLITERACY", "\n\nMEMORY", "\n\nWIDGETS"];
        for (flavor, base) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let session = |branch, base_ref, wake_section, cache_ttl| {
                    spawn_prompts(SpawnPromptInputs {
                        flavor: base.clone(),
                        coding_agent: agent,
                        workspace_sections: &workspace_sections,
                        branch,
                        base_ref,
                        repo_id: Some("3f9c1b2e"),
                        app_id: None,
                        wake_section,
                        cache_ttl,
                    })
                };
                let (one, one_context) = session("lucidos-one", None, "", None);
                let (two, two_context) = session(
                    "lucidos-two",
                    Some("origin/release"),
                    WAKE_DECISION_RULE,
                    Some(CacheTtl::OneHour),
                );
                assert_eq!(one, two, "{flavor}/{label}");
                assert!(!one.contains("lucidos-one"), "{flavor}/{label}");
                assert!(one.ends_with("\n\nLITERACY\n\nMEMORY\n\nWIDGETS"));

                let one_context = one_context.render().text;
                assert!(one_context.starts_with(SESSION_CONTEXT_HEADER));
                assert!(one_context.contains("on branch `lucidos-one`."));
                assert!(one_context.contains("(repo:3f9c1b2e:file#lucidos-one:src/main.rs#L5-L9)"));
                assert!(!one_context.contains("WAKE DECISION"), "{one_context}");
                let two_context = two_context.render().text;
                assert!(two_context.contains("`lucidos-two`, based on origin/release."));
                assert!(two_context.contains("WAKE DECISION"), "{two_context}");
                for context in [&one_context, &two_context] {
                    assert!(context.contains("PROMPT CACHE:"), "{flavor}/{label}");
                }
            }
        }
    }

    /// The lifetime rides the session context only, so the appended prompt
    /// stays byte-identical whatever cache a session gets (ADR 0511).
    #[test]
    fn the_background_rule_names_no_cache_lifetime() {
        for ttl in [CacheTtl::FiveMinutes, CacheTtl::OneHour] {
            for words in [ttl.lifetime_words(), ttl.blocking_cap_words()] {
                assert!(!BACKGROUND_PROCESS_RULE.contains(words), "{words}");
            }
        }
    }

    /// Claude Code states the observed lifetime, and 5 minutes when unknown.
    /// Codex states 5 minutes whatever was observed.
    #[test]
    fn the_cache_section_states_the_observed_lifetime() {
        use crate::runtime::CodingAgent::{ClaudeCode, Codex};
        let cases = [
            (Some(CacheTtl::OneHour), ClaudeCode, CacheTtl::OneHour),
            (
                Some(CacheTtl::FiveMinutes),
                ClaudeCode,
                CacheTtl::FiveMinutes,
            ),
            (None, ClaudeCode, CacheTtl::FiveMinutes),
            (Some(CacheTtl::OneHour), Codex, CacheTtl::FiveMinutes),
        ];
        for (observed, agent, stated) in cases {
            let section = prompt_cache_section(observed, agent);
            assert!(
                section.contains(&format!("lives {}.", stated.lifetime_words())),
                "{observed:?}/{agent:?}: {section}"
            );
            assert!(
                section.contains(&format!("under {}.", stated.blocking_cap_words())),
                "{observed:?}/{agent:?}: {section}"
            );
        }
    }

    /// The prompts that drop the branch tell the agent where to find it.
    #[test]
    fn fresh_flavors_point_at_the_session_context() {
        for (flavor, prompt) in [
            ("worktree", worktree_system_prompt("dev")),
            ("external_repo", external_repo_system_prompt("Acme")),
            (
                "app_worktree",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
            ),
        ] {
            assert!(prompt.contains("SESSION CONTEXT"), "{flavor}");
        }
    }

    /// An empty conditional section leaves no gap in the block.
    #[test]
    fn a_blank_session_context_section_is_skipped() {
        let mut context = SessionContext::default();
        context.push("BRANCH: one");
        context.push("\n\n  ");
        context.push("\n\nWAKE DECISION: two");
        assert_eq!(
            context.render().text,
            format!("{SESSION_CONTEXT_HEADER}\n\nBRANCH: one\n\nWAKE DECISION: two\n\n{SESSION_CONTEXT_END}")
        );
    }

    /// The e2e fake Claude Code parses the block with its own copies of these
    /// markers, since it cannot import them. This pins the copies.
    #[test]
    fn the_fake_claude_code_reads_the_same_session_context_markers() {
        let fake = include_str!("../../../../lucidos-app/e2e/fake-claude-code/claude.mjs");
        assert!(
            fake.contains(&format!("'{}'", SESSION_CONTEXT_END.replace('\n', "\\n"))),
            "claude.mjs lost the session context end marker"
        );
        assert!(
            fake.contains(&format!("/{BRANCH_SECTION_LEAD} `")),
            "claude.mjs lost the branch section lead"
        );
    }

    /// Only the chat card has a `message` field. A coding agent told to use it
    /// would write a field its card tool drops.
    #[test]
    fn coding_agent_card_rules_never_name_the_chat_message_field() {
        for rule in [ASK_USER_QUESTION_RULE, CODEX_ASK_USER_QUESTION_RULE] {
            assert!(!rule.contains("`message`"), "{rule}");
        }
    }

    fn lane_member<'a>(title: &'a str, sha: &'a str) -> LaneMemberBrief<'a> {
        LaneMemberBrief {
            title,
            summary: "Adds the thing.\n\nLonger notes nobody needs here.",
            branch: "lucidos-claude-code-repo-lucidos-x",
            snapshot_sha: sha,
        }
    }

    /// One landing merge per member, in lane order, each naming its exact
    /// snapshot and its change, then one `/harden`.
    #[test]
    fn lane_prompt_merges_each_member_as_its_own_landing_merge_in_order() {
        let prompt = build_lane_thread_prompt(&[
            lane_member("First fix", "aaa111"),
            lane_member("Owner's tweak", "bbb222"),
        ]);
        let first = prompt
            .find("git merge --no-ff aaa111 -m 'Land: First fix'")
            .expect("the first member merges its snapshot");
        let second = prompt
            .find(r"git merge --no-ff bbb222 -m 'Land: Owner'\''s tweak'")
            .expect("a quote in a title stays one shell word");
        assert!(first < second, "lane order: {prompt}");
        assert!(prompt.contains("These 2 changes"));
        assert!(prompt.contains("What it does: Adds the thing."));
        assert!(!prompt.contains("Longer notes"), "only the first paragraph");
        assert_eq!(prompt.matches("/harden").count(), 1, "{prompt}");
        assert!(prompt.contains("question tool"));
        assert!(prompt.contains("Never rebase"));
        assert!(prompt.contains("never touch a source thread's worktree"));
    }

    #[test]
    fn lane_prompt_for_one_member_reads_singular() {
        let prompt = build_lane_thread_prompt(&[lane_member("Only fix", "ccc333")]);
        assert!(prompt.starts_with("This change would not merge cleanly"));
        assert!(prompt.contains("git merge --no-ff ccc333 -m 'Land: Only fix'"));
    }

    /// The catch-up turn names what main gained and how to merge it, and
    /// lists a bounded number of commits.
    #[test]
    fn lane_catch_up_prompt_names_main_and_its_new_commits() {
        let commits: Vec<String> = (0..25).map(|n| format!("c{n} Land: change {n}")).collect();
        let prompt = build_lane_catch_up_prompt("mainsha", &commits);
        assert!(prompt.contains("- c0 Land: change 0"));
        assert!(!prompt.contains("c24"), "the list is bounded");
        assert!(prompt.contains("- and 5 more"));
        assert!(prompt.contains("git merge --no-ff mainsha -m 'Catch up with main'"));
        assert!(prompt.contains("end your turn"));
    }

    /// The lane thread asks rather than guesses, and never rebases.
    #[test]
    fn lane_system_prompt_keeps_the_landing_merges_and_asks_for_judgment() {
        let prompt = lane_thread_system_prompt();
        assert!(prompt.contains("question tool"));
        assert!(prompt.contains("Never rebase"));
        assert!(prompt.contains("/harden"));
        assert!(prompt.contains("landing merge"));
    }

    /// The Lucidos-source merge prompt gates the merge on `/harden`, which picks
    /// the suites for what changed. A fixed `make test` beside it ran the Rust
    /// suite for a CSS-only merge, after `/harden` had already tested it.
    #[test]
    fn merge_prompt_lucidos_source_keeps_harden_without_fixed_suites() {
        let prompt = build_merge_prompt("main", None, Some("desc"), false);
        assert!(prompt.contains("git merge main"));
        assert!(prompt.contains("/harden"));
        assert!(prompt.contains("desc"));
        assert!(
            !prompt.contains("make test"),
            "harden selects suites, the prompt must not; got: {prompt}",
        );
        assert!(
            !prompt.contains("npm test"),
            "harden selects suites, the prompt must not; got: {prompt}",
        );
    }

    /// Regression: the *app* merge prompt must NOT tell the app session to run
    /// `/harden` or the Lucidos-source test suites — app worktrees have none of
    /// that tooling. This is the CC-assisted (diverged-main) counterpart of the
    /// `apply_now` pre-merge harden-gate skip; without it, an app apply whose
    /// `main` diverged still injected `/harden` + `make test` into the app
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
            !prompt.contains("make test"),
            "app merge prompt must not mention make test; got: {prompt}",
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
        // Keep marker machinery in the plan rule. Left unnamed here, a
        // "then run `lucidos planned approve`" clause rides into external
        // prompts through the shared, environment-generic
        // `ASK_USER_QUESTION_RULE`. That is the same leak
        // `APPLY_CONFIRMATION_NOTE` was split out to prevent.
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
    fn app_worktree_prompt_inlines_the_manifest() {
        let manifest = r#"{"name":"Habit Tracker","icon":"assets/icon.svg"}"#;
        let prompt = app_worktree_system_prompt("dev", "habit-tracker", manifest);
        assert!(
            prompt.contains("`habit-tracker`"),
            "must name the app id so CC knows which folder it owns",
        );
        assert!(
            prompt.contains("dev"),
            "must name the workspace so cross-workspace context is clear",
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
    fn app_worktree_recovery_prompt_names_the_app() {
        let prompt = app_worktree_recovery_system_prompt("dev", "habit-tracker");
        assert!(prompt.contains("`habit-tracker`"));
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
        let prompt = external_repo_system_prompt("Acme");
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
        let prompt = external_repo_system_prompt("Acme");
        assert!(
            prompt.contains("git branch -m <new-name>"),
            "external prompt must tell CC to RENAME its branch in place, not fork a sibling",
        );
        assert!(
            !prompt.contains("Create feature branches"),
            "external prompt must not invite CC to create sibling feature branches",
        );
    }

    #[test]
    fn external_repo_recovery_prompt_omits_lucidos_only_tokens() {
        let prompt = external_repo_recovery_system_prompt("Acme");
        assert_no_lucidos_only_tokens(&prompt, "external_repo_recovery_system_prompt");
    }

    #[test]
    fn recovery_prompts_carry_restart_not_rejection_note() {
        // Every flavor of post-restart resume must tell the agent that a denial /
        // interrupted tool call in its transcript is a restart artifact, not the
        // user rejecting its approach — otherwise the resumed session changes
        // course on a phantom rejection.
        let cases: &[(&str, String)] = &[
            ("recovery_system_prompt", recovery_system_prompt("dev")),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("dev", "habit-tracker"),
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
            &external_repo_recovery_system_prompt("Acme"),
            "external_repo_recovery_system_prompt (with restart note)",
        );
    }

    #[test]
    fn external_prompts_keep_pkill_prevention() {
        // Slimmer process-safety rule still has to ban broad pkill — that
        // is the whole reason the rule exists; the script alternatives are
        // just bonus info that doesn't apply outside the Lucidos source.
        let prompt = external_repo_system_prompt("Acme");
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
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            (
                "external_repo_system_prompt",
                external_repo_system_prompt("Acme"),
            ),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme"),
            ),
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("dev", "habit-tracker"),
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
        // tears down every process it started, so a detached job still running
        // as the turn ends is killed, and nothing wakes the agent. Without this
        // guidance a DMG-build thread restarted its build three times and never
        // produced an artifact. Every chat-style prompt must carry the warning;
        // the merge-conflict prompt (no builds) deliberately omits it.
        let cases: &[(&str, String)] = &[
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            (
                "external_repo_system_prompt",
                external_repo_system_prompt("Acme"),
            ),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme"),
            ),
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery_system_prompt",
                app_worktree_recovery_system_prompt("dev", "habit-tracker"),
            ),
        ];
        for (label, prompt) in cases {
            for needle in [
                "BACKGROUND PROCESSES DON'T SURVIVE A TURN",
                "run_in_background",
                // The three waits that work must all survive a rewrite. The
                // foreground call names its ceiling, or it degrades into the
                // 120000 ms default that cuts a build off. The ceiling stays
                // inside the 5-minute prompt cache.
                "FOREGROUND",
                "`timeout: 240000`",
                // The cap points at the lifetime the session context states.
                "KEEP EVERY BLOCKING CALL INSIDE YOUR PROMPT CACHE",
                "SESSION CONTEXT says how long it lives",
                // The polled task keeps the turn, and each wait returns inside
                // the cache window.
                "lucidos background-task run --poll",
                "lucidos background-task wait <id>",
                // The watched task is the event-wait answer for long work: the
                // engine re-opens the thread, so the agent ENDS its turn.
                "WATCHED BACKGROUND TASK",
                "END YOUR TURN",
                "`unwatched`",
                // A long log re-read into context is the other cost, so the
                // redirect stays. Pinned by its instruction, not the example.
                "REDIRECT a chatty command's output to a log file",
                // Claude Code runs with background mode off (ADR 0358). The
                // model must learn that a timeout kills, not that it waits.
                "background mode OFF",
                "killed rather than moved to the background",
                // A subagent is background work. Without this arm the rule
                // reads as Bash-only, and that gap once produced a filler
                // subagent and then a fabricated question. Pin the blocking
                // call, the one-message fan-out that keeps it cheap, and the
                // ban on improvising a stall.
                "SUBAGENTS ARE BACKGROUND WORK TOO",
                "every `Agent` call blocks",
                "single assistant message",
                "NEVER improvise a stall",
                "fabricated question",
                // A subagent blocks mid-turn, past the keep-alive's reach
                // (ADR 0511). Long delegated work goes to a child thread.
                "counts against the cap",
                "slowest agent",
                "`lucidos spawn-thread --relation child`",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must warn that background processes die at turn end (`{needle}`)",
                );
            }
            // With background mode off, Claude Code rejects the parameter even
            // as `false`, so a prompt asking for it breaks every call.
            for banned in ["run_in_background: false", "run_in_background: true"] {
                assert!(
                    !prompt.contains(banned),
                    "{label} must not ask for `{banned}`: the parameter no longer exists",
                );
            }
            // A 10-minute foreground wait outlives the prompt cache, and the
            // wait the rule names returns within the engine's own cap.
            assert!(
                !prompt.contains("600000"),
                "{label} still prescribes a 10-minute wait"
            );
            let cap = crate::engine::agent_session::background_task::AGENT_WAIT_MAX_SECS;
            assert!(
                prompt.contains(&format!("returns within {cap} s")),
                "{label} must name the wait's {cap} s cap"
            );
        }

        // The rule lives in the shared base, so both backends inherit it — the
        // appended backend section can't strip it.
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            let full = append_backend_rules(worktree_system_prompt("dev"), agent);
            assert!(
                full.contains("BACKGROUND PROCESSES DON'T SURVIVE A TURN"),
                "worktree_system_prompt must keep the background-process rule for {:?}",
                agent,
            );
            // The subagent arm rides the same shared base. Codex has no
            // subagent tool, so for it these bytes are inert rather than wrong.
            // One copy is what stops the two backends drifting on what a
            // background wait means.
            assert!(
                full.contains("SUBAGENTS ARE BACKGROUND WORK TOO"),
                "worktree_system_prompt must keep the subagent arm for {:?}",
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

    /// I12 for coding agents: an undeclared sub-thread's session learns the CLI
    /// verb, and a parent learns what each wake mode does to its next report.
    #[test]
    fn coding_agents_are_told_how_a_report_reaches_the_parent() {
        for needle in [
            "lucidos wake-parent yes|no",
            "report is filed",
            "A failure always wakes it",
        ] {
            assert!(WAKE_DECISION_RULE.contains(needle), "missing {needle:?}");
        }
        for needle in [
            "`no-wake` files every report",
            "`--wake-mode wake`, the default",
            // ADR 0473: work this thread will not act on is a top-thread.
            "Add `--relation child` only to act on its result",
        ] {
            assert!(CODEX_CLI_RULE.contains(needle), "missing {needle:?}");
        }
    }

    /// Byte ceiling for each assembled system prompt, measured AFTER
    /// `append_backend_rules` because that is what a session actually receives.
    ///
    /// The engine-side counterpart of `scripts/check-context-budget.sh`, which
    /// gates the OTHER unconditional surface (`CLAUDE.md` plus the unscoped
    /// `.claude/rules/*.md`) and deliberately does not look at this one. Two
    /// surfaces reach every session before it has read a line of code, and
    /// both must be gated or this file grows forever without anything
    /// objecting. `docs/agent-config.md` § Which surface owns a rule says which
    /// content belongs here at all.
    ///
    /// A RATCHET, the same convention as `CONTEXT_BUDGET_CEILING`: lowering a
    /// number is the point and needs no ceremony, raising one needs a reason in
    /// the commit message saying what became worth paying for on every request
    /// of every session of that flavor.
    ///
    /// Per BACKEND as well as per flavor, because the two tails differ enough
    /// to hide each other. Codex swaps `ASK_USER_QUESTION_RULE` for a shorter
    /// one and then appends `CODEX_CLI_RULE` +
    /// `CODEX_SLASH_COMMANDS_RULE`, while Claude Code appends
    /// `PERMISSION_CONFIG_RULE`. One shared number per flavor would let either
    /// backend grow into the other's slack unnoticed.
    ///
    /// Backend labels are `CodingAgent::as_str()` values, kebab-case, so the
    /// table reads the same as every other public surface naming a backend.
    /// EVERY row rose by about 500 bytes for `SHOWING_AN_IMAGE_RULE`. It rides
    /// the shared chokepoint, so no flavor can opt out, and none should: a
    /// screenshot read into the agent's own context reaches the user on no
    /// backend and in no worktree shape.
    const PROMPT_FLAVOR_CEILINGS: &[(&str, &str, usize)] = &[
        // EVERY row rose by 71 bytes when `REASONING_NOT_VISIBLE_RULE` said the
        // question renders markdown. A card of findings arrived as one run-on
        // paragraph without it.
        //
        // EVERY row rose by 234 to 297 bytes for `HTML_ARTIFACT_TYPE_SCALE_RULE`
        // (ADR 0319). It rides the shared chokepoint: a session that never
        // loaded the skill wrote HTML at a 16px scale.
        //
        // The twelve chat-style rows rose for the DANGLING ITEM paragraph in
        // the ask rule: Claude Code by 758 bytes, Codex by 422. The two
        // conflict_resolution rows stay put, because they carry no ASKING
        // USERS section and so no ask rule.
        //
        // The four Lucidos-source rows (worktree + recovery, both backends)
        // are 549 bytes higher than they were, for `NO_MEMORY_FILES_RULE`.
        // Only these flavors carry it: a fact about THIS repo has to land in
        // git, where Codex and the next session on another machine read it.
        //
        // Those four rows and both conflict_resolution rows are 257 bytes
        // higher for `PERMISSION_ASK_RULE`. The other rows had the slack.
        //
        // The same four rose by 185 more when `APPLY_RESTART_RULE` learned
        // that every file a binary embeds requires a restart. An agent that
        // misses it promises an Apply the running engine never picks up.
        //
        // EVERY row carries `READ_DECISION_RULE` (ADR 0409, ADR 0417), under
        // 200 bytes. Six rows had no slack and rose by it: these four and both
        // conflict_resolution rows. Without it the engine forces every
        // session's read decision, which costs an auxiliary call per turn.
        //
        // EVERY row carries the widget embed sentence in `SHOWING_AN_IMAGE_RULE`
        // (ADR 0415), 85 bytes. Six rows had no slack and rose by it: these four
        // and both conflict_resolution rows. The embed is how an option carries
        // a sample the user must hear or try, and it rides the picture rule.
        //
        // EVERY Codex row rose by 103 to 119 bytes when `CODEX_CLI_RULE` said a
        // child's clean report may be filed rather than re-open the thread. A
        // Codex parent otherwise waits for a wake that never comes.
        //
        // EVERY row rose by 208 to 239 bytes for `SUCCESSOR_THREAD_RULE`. It
        // is what turns "handoff this session", and advice to `/clear`, into
        // a successor thread instead of a lost context.
        //
        // The Codex rows rose by 42 more, and two Lucidos-source rows by 3.
        // Main grew `CODEX_CLI_RULE` and the plan rule beside the successor
        // rule, and neither side left slack for the other.
        //
        // Most rows rose by up to 123 bytes for the large-thread plan card.
        // The plan and ask rules now say it leads with `Implement in a new
        // thread`, as the approval checkpoint prints it.
        //
        // EVERY row rose by 34 bytes for named checkpoints (ADR 0505). The
        // successor rule names `--at <name>` and the knowhow that wires one,
        // since the engine no longer knows any checkpoint by name. The plan
        // rule's shorter pointer paid back 28 on the Lucidos-source rows.
        //
        // The twelve non-conflict rows rose by 173 bytes when the background
        // rule counted a subagent against the cache (ADR 0511). A long one
        // rewrote the parent's whole context, in every repo.
        //
        // EVERY row rose by 314 to 324 bytes for `READ_NARROWLY_RULE`. Tool
        // output is most of a session's input cost, and the rule teaches
        // locate-then-window reads in every repository.
        //
        // Every row fell in the baseline trim: the plan, ask, apply and
        // permission-config rules each say a fact once, and the Lucidos
        // worktree flavor lost 2.6 KB.
        //
        // The claude-code rows rose by 12 bytes when `READ_DECISION_RULE` put
        // the call in its own message. An answer written beside the call was
        // written again after it, and the user read it twice.
        //
        // The twelve non-conflict rows rose for quiet options in the ask rule:
        // Claude Code by 181 bytes, Codex by 150. Without it an agent cannot
        // offer a "Not now" that leaves the thread asleep.
        ("worktree", "claude-code", 25603),
        ("worktree", "codex", 25541),
        // The four external-repo rows are 569 bytes higher than they were, for
        // `BUILD_SLOT_RULE` (ADR 0070). Only these flavors carry it. A
        // Lucidos-source session is already covered, because `make lint` and
        // `make test` take a slot themselves. Carrying it there would pay for
        // an instruction the session cannot use.
        ("external_repo", "claude-code", 18419),
        ("external_repo", "codex", 18357),
        ("recovery", "claude-code", 24320),
        ("recovery", "codex", 24258),
        ("external_repo_recovery", "claude-code", 18321),
        ("external_repo_recovery", "codex", 18259),
        ("app_worktree", "claude-code", 20911),
        ("app_worktree", "codex", 20849),
        ("app_worktree_recovery", "claude-code", 19471),
        ("app_worktree_recovery", "codex", 19409),
        // Both conflict_resolution rows rose by about 115 bytes when
        // `SHOWING_AN_IMAGE_RULE` learned that saving a picture shows nothing
        // either. The other rows had the slack to absorb it. Both rose again,
        // by 384, for its visual-choice clause: a real picture per option.
        // The external-repo, app-worktree and conflict-resolution rows rose by
        // 22 bytes when `READ_DECISION_RULE` excused a turn that asks a question.
        ("conflict_resolution", "claude-code", 7178),
        ("conflict_resolution", "codex", 9245),
    ];

    /// Both backends, paired with the label used in `PROMPT_FLAVOR_CEILINGS`.
    fn all_backends() -> [(crate::runtime::CodingAgent, &'static str); 2] {
        [
            (crate::runtime::CodingAgent::ClaudeCode, "claude-code"),
            (crate::runtime::CodingAgent::Codex, "codex"),
        ]
    }

    /// The engine system prompt is one of the two surfaces every session pays
    /// for before reading any code. This is its regrowth gate.
    ///
    /// Two arms, mirroring `check-context-budget.sh`. SIZE: no assembled prompt
    /// exceeds its ceiling. MEMBERSHIP: every flavor/backend pair has a
    /// declared ceiling and every declared ceiling names a real pair, so a
    /// renamed or deleted flavor cannot leave a dead row that silently gates
    /// nothing.
    #[test]
    fn every_prompt_flavor_stays_under_its_size_ceiling() {
        let mut table = Vec::new();
        let mut over = Vec::new();
        let mut seen = Vec::new();

        for (flavor, base) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let size = append_backend_rules(base.clone(), agent).len();
                seen.push((flavor, label));
                let ceiling = PROMPT_FLAVOR_CEILINGS
                    .iter()
                    .find(|(f, b, _)| *f == flavor && *b == label)
                    .map(|(_, _, c)| *c);
                match ceiling {
                    Some(ceiling) => {
                        table.push(format!("{size:>7} / {ceiling:<7} {flavor}/{label}"));
                        if size > ceiling {
                            over.push(format!(
                                "{flavor}/{label}: {size} bytes, over its {ceiling} ceiling by {}",
                                size - ceiling
                            ));
                        }
                    }
                    None => over.push(format!(
                        "{flavor}/{label}: {size} bytes, no ceiling declared in PROMPT_FLAVOR_CEILINGS"
                    )),
                }
            }
        }

        for (flavor, label, _) in PROMPT_FLAVOR_CEILINGS {
            assert!(
                seen.iter().any(|(f, b)| f == flavor && b == label),
                "PROMPT_FLAVOR_CEILINGS declares {flavor}/{label}, which no longer exists. \
                 A dead row gates nothing, so drop it or fix the name."
            );
        }

        assert!(
            over.is_empty(),
            "the engine system prompt grew past its ceiling:\n  {}\n\nall flavors (bytes / ceiling):\n  {}\n\n\
             Every byte is paid on every request of every session of that flavor, with \
             system-prompt authority, before the agent has read a line of code. Prefer moving \
             the content: repo conventions belong in CLAUDE.md (see docs/agent-config.md \
             section \"Which surface owns a rule\"), and maintainer explanation belongs in \
             docs/agent-config.md, which never loads. Raising a ceiling is allowed and is a \
             deliberate act: say in the commit message what became worth paying for.",
            over.join("\n  "),
            table.join("\n  "),
        );
    }

    /// Every prompt flavor, built the way the real spawn builds it (before
    /// `append_backend_rules`). Shared by the guards for rules injected at that
    /// chokepoint, which must reach all of them for both backends.
    fn all_prompt_flavors() -> Vec<(&'static str, String)> {
        vec![
            ("worktree", worktree_system_prompt("dev")),
            ("external_repo", external_repo_system_prompt("Acme")),
            ("recovery", recovery_system_prompt("dev")),
            (
                "external_repo_recovery",
                external_repo_recovery_system_prompt("Acme"),
            ),
            (
                "app_worktree",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery",
                app_worktree_recovery_system_prompt("dev", "habit-tracker"),
            ),
            (
                "conflict_resolution",
                conflict_resolution_system_prompt().to_string(),
            ),
        ]
    }

    /// Every session, on both backends, learns to locate before it reads.
    #[test]
    fn every_session_is_taught_to_read_narrowly() {
        for (flavor, prompt) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let prompt = append_backend_rules(prompt.clone(), agent);
                assert!(
                    prompt.contains(READ_NARROWLY_RULE),
                    "{flavor}/{label} lacks the read-narrowly rule"
                );
            }
        }
        assert!(READ_NARROWLY_RULE.contains("rg -n"));
        assert!(READ_NARROWLY_RULE.contains("100 lines"));
    }

    /// Every session, on both backends, learns the one handoff path, and the
    /// rule names the CLI rather than any source of `/clear` advice.
    #[test]
    fn every_session_is_taught_the_successor_thread() {
        for (flavor, prompt) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let prompt = append_backend_rules(prompt.clone(), agent);
                assert!(
                    prompt.contains(SUCCESSOR_THREAD_RULE),
                    "{flavor}/{label} lacks the successor rule"
                );
            }
        }
        assert!(SUCCESSOR_THREAD_RULE.contains("lucidos successor spawn"));
        assert!(SUCCESSOR_THREAD_RULE.contains("`/clear`"));
        // A repository names its own checkpoints, so the rule sends the agent
        // to the command its flow names, and to the knowhow for wiring one.
        assert!(SUCCESSOR_THREAD_RULE.contains("HANDOFF CHECKPOINT"));
        assert!(SUCCESSOR_THREAD_RULE.contains("`lucidos successor offer --at <name>`"));
        assert!(SUCCESSOR_THREAD_RULE.contains("system-knowhow/coding-agent-flows"));
        // A warm Continue on the Successor card queues exactly this phrase.
        let request = crate::engine::successor::HANDOFF_REQUEST;
        assert!(SUCCESSOR_THREAD_RULE.contains(&format!("\"{request}\"")));
    }

    #[test]
    fn only_external_repo_sessions_are_taught_the_build_slot_wrapper() {
        // The wrapper is how a session in SOMEBODY ELSE'S repo joins the pool:
        // nothing there wraps a build for it. A Lucidos-source or app session
        // must NOT carry it. Those builds go through `make` and the build
        // scripts, which take a slot already. The text would be bytes on every
        // request buying nothing (ADR 0070).
        let taught = ["external_repo", "external_repo_recovery"];
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &all_prompt_flavors() {
                let full = append_backend_rules(base.clone(), agent);
                let has = full.contains("HEAVY BUILDS TAKE A BUILD SLOT");
                assert_eq!(
                    has,
                    taught.contains(label),
                    "{label}/{} carries the build-slot rule: {has}, expected the opposite",
                    agent.as_str()
                );
                if has {
                    assert!(
                        full.contains("lucidos build-slot -- "),
                        "{label} must show the wrapper's exact prefix, not just name it"
                    );
                }
            }
        }
    }

    #[test]
    fn only_lucidos_source_sessions_are_told_not_to_write_memory_files() {
        // Knowledge about this repo belongs in git, where Codex and the next
        // session on another machine can read it. That is OUR working
        // agreement, so the rule is scoped to the two Lucidos-source flavors.
        // An external repo has its own conventions, and an app worktree is the
        // user's workspace git. Both backends carry it: the standard is about
        // where the fact lands, not which agent wrote it.
        let taught = ["worktree", "recovery"];
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &all_prompt_flavors() {
                let full = append_backend_rules(base.clone(), agent);
                let has = full.contains("NEVER WRITE A CODING-AGENT MEMORY FILE");
                assert_eq!(
                    has,
                    taught.contains(label),
                    "{label}/{} carries the no-memory-file rule: {has}, expected the opposite",
                    agent.as_str()
                );
                if has {
                    // Naming the ban alone leaves the fact homeless, and an
                    // agent with a fact and no home writes the memory file.
                    for needle in ["memory/", "lucidos data write knowhow/", "`.claude/rules/`"] {
                        assert!(
                            full.contains(needle),
                            "{label} must name both the banned directory and where the \
                             fact goes instead (missing: {needle:?})"
                        );
                    }
                }
            }
        }
    }

    /// A session may not settle a question about the shipped product's
    /// security, data exposure, or user-visible correctness by itself. When it
    /// finds a user-reaching thing it will not fix, it must raise it with the
    /// question tool before finishing. It must not bury it as a non-goal in a
    /// plan file, an ADR, or a commit message. That gap shipped an
    /// unauthenticated app-frame reach and a clipboard regression (ADR 0227,
    /// ADR 0231).
    ///
    /// Scoped to the six chat-style prompts, the ones carrying the ASKING USERS
    /// section this rule points at. `conflict_resolution` has no such section
    /// and runs unattended, so it must NOT carry the rule. Both backends carry
    /// it: the rule is in the shared builder. It names no tool, so the Codex
    /// swap of the ASKING USERS section leaves it correct.
    #[test]
    fn chat_style_prompts_require_raising_user_reaching_findings() {
        let taught = [
            "worktree",
            "recovery",
            "external_repo",
            "external_repo_recovery",
            "app_worktree",
            "app_worktree_recovery",
        ];
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &all_prompt_flavors() {
                let full = append_backend_rules(base.clone(), agent);
                let has = full.contains("RAISE A USER-REACHING FINDING YOU WON'T FIX");
                assert_eq!(
                    has,
                    taught.contains(label),
                    "{label}/{} carries the raise-findings rule: {has}, expected the opposite",
                    agent.as_str()
                );
                if has {
                    // The load-bearing bar must survive a paraphrase, the same
                    // way APPLY_RESTART_RULE pins its file-type list. Without
                    // these, an edit could keep the header and gut the rule.
                    for needle in [
                        // Burying it does not discharge the obligation.
                        "does NOT discharge this",
                        // A note is not a gate; the authority claim.
                        "your authority stops at the product's users",
                        // Reach is judged by population and harm, not by
                        // whether it fires on this machine.
                        "NOT by whether it fires on this machine or workspace",
                        // The carve-out that keeps it from firing on trivia.
                        "A finding you DO fix in this change needs no question",
                    ] {
                        assert!(
                            full.contains(needle),
                            "{label}/{} must keep the raise-findings bar (missing: {needle:?})",
                            agent.as_str()
                        );
                    }
                    // The rule points at the ASKING USERS section for the
                    // question tool. That pointer dangles unless the section is
                    // in the same prompt. Pin it, like the plan-rule test pins
                    // its own pointer target.
                    assert!(
                        full.contains("ASKING USERS:"),
                        "{label}/{} names the ASKING USERS section, so it must be present",
                        agent.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn coding_agent_prompts_ask_for_what_and_why_on_a_command() {
        // The permission card leads with this line, so every flavor on both
        // backends must ask the agent to write it for the user.
        let flavors = all_prompt_flavors();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &flavors {
                let full = append_backend_rules(base.clone(), agent);
                assert!(
                    full.contains("SAY WHAT A COMMAND DOES AND WHY")
                        && full.contains("Write it for the user: what it does and"),
                    "{label} ({agent:?}) must ask for what and why on a command",
                );
            }
        }
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
                    "only as a short note of a sentence or two",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must tell the agent its reasoning is not shown (`{needle}`)",
                    );
                }
                // The Vertex relay makes notes before a tool call visible, so
                // the stopgap that sent them into the card must stay gone.
                assert!(
                    !full.contains("just before a tool call"),
                    "{label} ({agent:?}) still carries the hidden-notes stopgap",
                );
            }
        }
        // The backend-independent prepend must not have broken the Codex
        // ask-rule swap that `append_backend_rules` performs after it.
        let codex = append_backend_rules(
            worktree_system_prompt("dev"),
            crate::runtime::CodingAgent::Codex,
        );
        assert!(
            codex.contains("ask_user_question` tool (on the `lucidos` MCP"),
            "Codex prompt must still swap in the MCP ask_user_question rule",
        );
    }

    /// An agent that wrote its answer beside the read decision's call then
    /// wrote it again after the call, and the user read it twice.
    #[test]
    fn the_read_decision_comes_before_the_answer_in_every_flavor() {
        for (label, base) in all_prompt_flavors() {
            for (backend, backend_label) in all_backends() {
                let built = append_backend_rules(base.clone(), backend);
                assert!(
                    built.contains("in its own message, then answer once"),
                    "{label}/{backend_label}"
                );
            }
        }
    }

    /// Notes that never reach the user: the agent's own context holds the text
    /// it wrote, so only the prompt can tell it the user saw none.
    #[test]
    fn a_hidden_notes_session_reads_the_hard_rule_in_place_of_the_note_sentence() {
        use crate::runtime::claude_code_provider::NotesVisibility::{Hidden, Shown};
        for (label, base) in all_prompt_flavors() {
            let built = append_backend_rules(base, crate::runtime::CodingAgent::ClaudeCode);
            let hidden = with_notes_visibility(built.clone(), Hidden);
            assert!(hidden.contains(NOTES_HIDDEN_SENTENCE), "{label}");
            assert!(hidden.contains("NEVER reach the user"), "{label}");
            // Text before a tool call can still show, so a rule saying it never
            // does made agents repeat their whole answer after the call.
            assert!(hidden.contains("never write it before a tool call and again after"));
            assert!(
                !hidden.contains(NOTES_REACH_USER_SENTENCE),
                "{label}: both rules in one prompt"
            );
            assert!(
                !hidden.contains("only as a short note"),
                "{label}: a hidden session is never told the note arrives"
            );
            // The rest of the rule stays, so the card advice still follows.
            assert!(hidden.contains("YOUR REASONING IS NOT SHOWN TO THE USER"));
            assert!(hidden.contains("goes on the question card itself"));
            assert_eq!(
                with_notes_visibility(built.clone(), Shown),
                built,
                "{label}: a shown session keeps today's prompt byte for byte"
            );
            assert!(!built.contains(NOTES_HIDDEN_SENTENCE), "{label}");
        }
    }

    /// A Claude Code session asked for "a showcase html" wrote its own 16px
    /// scale with headings up to `4rem`. The type scale lived only in a skill
    /// it never loaded (ADR 0319). Every flavor on both backends must
    /// carry the three facts that decide it, and where the full scale lives.
    #[test]
    fn coding_agent_prompts_carry_the_html_artifact_type_scale() {
        let flavors = all_prompt_flavors();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &flavors {
                let full = append_backend_rules(base.clone(), agent);
                for needle in [
                    "HTML ARTIFACT TYPE SCALE",
                    "body `0.75rem`",
                    "`1rem` is a heading",
                    "never size the root",
                    "`lucidos knowhow read system-knowhow/best-practices`, § Standalone HTML",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must carry the HTML artifact type scale (`{needle}`)",
                    );
                }
            }
        }
    }

    /// Regression guard for the session that screenshotted a docs page, called
    /// `Read` on the PNG, and told the user "I did send an image". Reading an
    /// image feeds the agent's own context and reaches the user not at all, so
    /// every flavor must carry both halves: that `Read` shows them nothing, and
    /// what to do instead. Rides the same `append_backend_rules` chokepoint as
    /// the reasoning rule.
    #[test]
    fn coding_agent_prompts_say_reading_an_image_does_not_show_it() {
        let flavors = all_prompt_flavors();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for (label, base) in &flavors {
                let full = append_backend_rules(base.clone(), agent);
                for needle in [
                    "READING OR SAVING AN IMAGE DOES NOT SHOW IT TO THE USER",
                    // The alternative is the load-bearing half: the session
                    // that failed did not know one existed.
                    "lucidos data write artifacts/",
                    "![what it shows](artifacts/x.png)",
                    // The tell names the step's subject, see the rule's doc.
                    "their step records that you read a file",
                    // A session saved the picture, wrote "I've drawn the
                    // options", and pasted no image line, so saving must be
                    // named as showing nothing too.
                    "only stores it, and the user still sees nothing",
                    // Codex's question tool takes plain string options, so an
                    // unconditional `preview` would send it objects it drops.
                    "`preview` if your tool has one",
                    // Pictures pasted into the reply just before a card were
                    // summarized away, twice. So the card comes first, with
                    // the reason, and the reply only when it ends the turn.
                    "Before a card, put it ON the question card",
                    "arrive as a short summary that drops it",
                    "in the reply ending your turn",
                    // Colour options came as ASCII art, then as one sheet of
                    // all four options repeated on every button.
                    "ASCII art shows no colour",
                    "its own picture of only that option",
                    "Never repeat one combined sheet on every option",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must say reading an image shows the user nothing, \
                         and how to show one (`{needle}`)",
                    );
                }
                // "The same message" as the card is where the picture is lost.
                assert!(
                    !full.contains("without that line in the same message"),
                    "{label} ({agent:?}) must not send a picture into the words before a card",
                );
            }
        }
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
            ("worktree", worktree_system_prompt("dev")),
            ("recovery", recovery_system_prompt("dev")),
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
            worktree_system_prompt("dev"),
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
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            (
                "external_repo_system_prompt",
                external_repo_system_prompt("Acme"),
            ),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
            (
                "external_repo_recovery_system_prompt",
                external_repo_recovery_system_prompt("Acme"),
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
            // prose and the thread sits idle until the user types "approve"
            // by hand. The carve-out must survive, with the concrete option
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
            // A real fork satisfies the two-option minimum on its
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
                prompt.contains("Tapping a fork is still an approval"),
                "{label} must say a fork answer approves that variant rather than rejecting \
                 the plan (without naming the plan marker, which is Lucidos-source only)",
            );
            // A prose question ends the turn, so the thread looks finished and
            // nothing tells the user someone is waiting. Only the tool call
            // parks it in `WaitingForUserAnswer`, which drives the
            // Blocked badge, Review routing, and the notification
            // trigger. Pin the general form, not just the plan-approval case.
            assert!(
                prompt.contains("WHY THE TOOL AND NOT PROSE"),
                "{label} must say that only the tool marks the thread as blocked, \
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
            // A third "don't ask" case, distinct from the post-work
            // confirmation above. One session held its turn open by asking
            // "placeholder - not a real question, will not be sent". That label
            // is the belief to kill: it expected a discard path. Pin the ban
            // AND the mechanism, since a ban whose reason is dropped gets
            // re-derived away by the next model.
            for needle in [
                "NEVER FABRICATE A QUESTION TO STALL",
                "There is no discard path",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must forbid a question asked only to stall, and say why it \
                     cannot work (missing: {needle:?})",
                );
            }
        }

        // `append_backend_rules` swaps the whole rule for the Codex variant, so
        // a ban written once reaches one backend. Both copies carry it.
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            let full = append_backend_rules(worktree_system_prompt("dev"), agent);
            assert!(
                full.contains("NEVER FABRICATE A QUESTION TO STALL"),
                "the stall ban must survive the {:?} rule swap",
                agent,
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
            ("worktree", worktree_system_prompt("dev")),
            ("recovery", recovery_system_prompt("dev")),
            (
                "app_worktree",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
            ),
            (
                "app_worktree_recovery",
                app_worktree_recovery_system_prompt("dev", "habit-tracker"),
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
                // The change is appliable from the thread the user reads the
                // report in. An agent told nothing guesses a panel and names it.
                assert!(
                    full.contains("Never tell the user where to apply it"),
                    "{label} ({agent:?}) must forbid naming where to apply the change",
                );
                assert!(
                    !full.contains("Apply panel"),
                    "{label} ({agent:?}) must not name a panel the user applies from",
                );
            }
        }

        // External-repo prompts must NOT mention Apply / change-proposal — they
        // push and open PRs. Guards the exact leak the split fixes.
        let external: &[(&str, String)] = &[
            ("external_repo", external_repo_system_prompt("Acme")),
            (
                "external_repo_recovery",
                external_repo_recovery_system_prompt("Acme"),
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
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
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
                // "approve" by hand. The tool is named indirectly
                // ("the ASKING USERS section") because the two backends call it
                // different things and `append_backend_rules` swaps only that
                // section, never this rule.
                "ASK FOR APPROVAL WITH THE QUESTION TOOL",
                "`Approve` and `Request changes`",
                // The pair is a FLOOR: a plan that offers a real fork puts the
                // fork in the second slot instead of pushing `Request changes`
                // to a third, where it would mean only "I will type what I want
                // changed". And a fork answer is an approval, so
                // the rule that owns `lucidos planned approve` has to say the
                // agent may flip the marker after revising the plan to match.
                "make that fork the second option",
                "A tapped fork is an approval too",
                // A message that also calls a tool never reaches the user, so
                // the plan summary rides on the card itself.
                "the card's question carries a short markdown summary",
                // A typed reply that asks is a request for changes. The engine
                // refuses `planned approve` without the user's own pick.
                "A typed reply that asks, suggests or questions is a request for changes",
                // The skill's approval checkpoint prints the card's shape
                // from the handoff estimate, so the rule only points there.
                "Offer the options the skill's approval checkpoint prints",
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
                external_repo_system_prompt("Acme"),
            ),
            (
                "app_worktree_system_prompt",
                app_worktree_system_prompt("dev", "habit-tracker", "{}"),
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
    /// idle until the user typed "approve" by hand.
    ///
    /// Each backend must name its OWN tool: `append_backend_rules` swaps the
    /// whole `ASK_USER_QUESTION_RULE` for the Codex variant, so a CC tool name
    /// leaking into a Codex prompt would send it after a tool it cannot call.
    #[test]
    fn plan_approval_is_carved_out_of_the_no_confirmations_rule_on_both_backends() {
        let base = worktree_system_prompt("dev");
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
            // button was the dead-end shape the same rule bans.
            for needle in [
                "make that fork the second option",
                "Tapping a fork is still an approval",
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
        let prompt = worktree_system_prompt("dev");
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
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
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
            "`crates/lucidos-gateway/`",
            "`crates/lucidos-app/src/styles/global/shared-components.css`",
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
            // Apply only builds; the user switches. The retired label reads
            // as a restart on click, so it must not come back.
            assert!(
                !prompt.contains("Apply & Restart"),
                "{label} must not name the retired \"Apply & Restart\" label",
            );
            for needle in [
                "\"Apply*\"",
                "Apply never restarts Lucidos",
                "Switch to new version",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must say {needle}: Apply builds, the user switches",
                );
            }
        }
    }

    /// Claude Code no longer has `TaskOutput`, `TaskUpdate` or `TaskList`. A
    /// prompt naming one sends the agent to a tool that answers "No such tool
    /// available", after which it improvises a wait. No flavor may name them,
    /// for either backend.
    #[test]
    fn no_prompt_names_a_task_tool_claude_code_removed() {
        for (label, prompt) in all_prompt_flavors() {
            for (agent, backend) in all_backends() {
                let full = append_backend_rules(prompt.clone(), agent);
                for gone in ["TaskOutput", "TaskUpdate", "TaskList"] {
                    assert!(
                        !full.contains(gone),
                        "{label} for {backend} must not name `{gone}`, which Claude Code removed"
                    );
                }
            }
        }
    }

    /// The Codex teaching section must be applied for Codex and ONLY for
    /// Codex: CC sessions already get the lucidos-cli skill file + the native
    /// `AskUserQuestion` tool, so duplicating the teaching there wastes
    /// context; a Codex session without it can't land workspace `data/` files
    /// (the sandbox blocks direct writes) and guesses instead of asking.
    #[test]
    fn codex_prompts_carry_cli_and_question_teaching() {
        let base = worktree_system_prompt("dev");
        let codex = append_backend_rules(base.clone(), crate::runtime::CodingAgent::Codex);
        for needle in [
            // The header discriminates and cannot drift: it is the one needle
            // no shared rule can satisfy. `lucidos data write` and
            // `lucidos spawn-thread` no longer discriminate on their own,
            // because `SHOWING_AN_IMAGE_RULE` and `NAMES_NOT_IDS_RULE` name
            // them for both backends. They stay to pin the section's contents.
            "LUCIDOS CLI: The `lucidos` CLI is on your PATH",
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
        // The sentinels are the SECTION HEADER plus two subcommands only it
        // teaches, never a command it happens to mention. `lucidos data write`
        // used to stand in for the section and stopped being unique to it:
        // `SHOWING_AN_IMAGE_RULE` names the same command for every backend,
        // because writing an artifact is how any agent shows a picture. The
        // body needles matter because a header-only check passes if the CLI
        // teaching is ever split across two consts and CC picks up the second.
        // `lucidos spawn-thread` is NOT eligible: `NAMES_NOT_IDS_RULE` names it
        // on both backends.
        for needle in [
            "LUCIDOS CLI: The `lucidos` CLI is on your PATH",
            "lucidos events emit",
            "lucidos await-event",
        ] {
            assert!(
                !cc.contains(needle),
                "the CC prompt must not duplicate the Codex CLI teaching ({needle:?})",
            );
        }
    }

    /// The permission-config rule (`--allowedTools` / `cc-allowed-tools`
    /// mechanics) is Claude-Code-only and must be appended for CC and ONLY for
    /// CC — mirroring how [`CODEX_CLI_RULE`] is Codex-only. Codex
    /// permissions surface through its own sandbox + approval-policy model
    /// (approval cards raised by the app-server), so the CC mechanics are
    /// misleading noise (and wasted tokens) on a Codex session. The shared
    /// base prompt must carry NEITHER backend's section: `append_backend_rules`
    /// is the single split point.
    #[test]
    fn permission_config_rule_is_claude_code_only() {
        let base = worktree_system_prompt("dev");
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
            "<workspace>/.lucidos/cc-allowed-tools",
            "PER WORKSPACE",
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
