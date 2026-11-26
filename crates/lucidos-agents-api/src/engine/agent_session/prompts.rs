use crate::engine::cache_ttl::{CacheLifetime, CacheTtl};
use crate::runtime::agent_notes::notes_reach_user_sentence;

pub(super) fn workspace_preamble(workspace_name: &str) -> String {
    format!(
        "WORKSPACE: You were spawned by the \"{workspace_name}\" Lucidos workspace. \
         When the user refers to threads, events, or data, they mean data in this workspace."
    )
}

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

const NO_PULL_REQUESTS_RULE: &str = "NO PULL REQUESTS: Lucidos is not a PR-based codebase. \
    Never run `gh pr create`, never `git push` your branch, never tell the user to \
    \"open a PR\" or \"submit a PR\". The engine is the merge mechanism: when the user clicks \
    Apply, your branch lands on main and is pushed to the remote in one step. Override CC's \
    default \"Creating pull requests\" guidance — it does not apply here.";

const COMMIT_CADENCE_RULE: &str = "COMMIT CADENCE: Commit completed, coherent slices of work \
    as you go, not only at the end. Use `git status` and `git diff` to review what will be \
    included before each commit. Do not commit after every tiny edit; do commit after each \
    self-contained fix, feature slice, cleanup checkpoint, or other reviewable unit so the \
    Diff view and recovery state stay current. Avoid committing known-broken work unless you \
    are explicitly checkpointing an intermediate state and the commit message says so.";

const NO_MEMORY_FILES_RULE: &str = "NEVER WRITE A CODING-AGENT MEMORY FILE: Do not write to \
    your agent's per-user memory directory (for Claude Code that is \
    `$CLAUDE_CONFIG_DIR/projects/<cwd>/memory/`, a `MEMORY.md` plus one file per fact). \
    Nothing else reads it: not Codex, not the user, not the next session on another machine. \
    Knowledge lives in git instead, committed in the same change so it merges. A convention \
    for agents goes in `CLAUDE.md` or `.claude/rules/`; a workspace fact goes in the \
    workspace's own knowhow, written with `lucidos data write knowhow/<topic>/<name>.md`.";

const APPLY_RESTART_RULE: &str = "APPLY/RESTART: After your session ends, your commits sit \
    as a pending change on this thread. The user explicitly clicks Apply to merge your branch into \
    main — nothing happens automatically. The button label is \"Apply\" (no restart needed) or \
    \"Apply*\" (restart needed); the engine derives this from the touched files. ANY \
    of these triggers restart: a non-test `.rs` file, `Cargo.toml`, `Cargo.lock`, a `.sql` \
    migration under `migrations/`, an SDK bundle source under `packages/lucidos-sdk/`, any \
    non-test, non-doc file in `crates/lucidos-engine/`, `crates/lucidos-gateway/` or `crates/lucidos-cli/` \
    (e.g. `crates/lucidos-agents-api/src/api/sdk_iframe.css`, `sdk_iframe_audio.js`), or a file a \
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

const IMPLEMENTATION_PLAN_RULE: &str = "IMPLEMENTATION PLAN: Before your FIRST code edit, decide \
    whether this is complex work — ADR- or design-thread-backed, cross-layer, any routing / \
    topology / storage / security / migration / process change, or anything beyond a local bug \
    fix. If it is, produce an implementation plan FIRST: run the `implementation-plan` skill \
    (`.claude/skills/implementation-plan/SKILL.md`). It writes `docs/plans/<date>-<slug>.md` and records a \
    PROPOSED plan marker via `lucidos planned mark --plan <path>`. A proposed plan does NOT \
    unblock editing: ASK FOR APPROVAL WITH THE QUESTION TOOL named in the ASKING USERS section, \
    offering `Approve` and `Request changes` in the shape that section gives. A message that also \
    calls a tool never reaches the user, so the card's question carries a short markdown summary \
    of the plan, ask last. Offer the options the skill's approval checkpoint prints, which may \
    swap this pair for `Implement in a fresh session`. \
    Once the user approves, run `lucidos planned approve` to flip the marker to gate-satisfying, which unblocks \
    edits and Apply. Then build in the same turn, unless the user picked a fresh session. Lucidos refuses it unless the user tapped an approving \
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

const RESTART_NOT_REJECTION_RULE: &str = "RESTART CONTEXT — NOT A REJECTION: This session was \
    resumed after the engine restarted mid-work. If your recent history shows a permission denial \
    (e.g. \"User denied\"), a tool call that was interrupted or never completed, or a synthetic \
    \"[Request interrupted]\" result, that was caused by the restart — NOT by the user rejecting \
    your work, your plan, or your approach. Do not abandon or rework your approach on account of \
    those signals. Re-confirm where you left off (the git log/diff steps above) and continue the \
    same plan, unless the user has since told you otherwise in a new message.";

const BUILD_SLOT_RULE: &str = "\n\nHEAVY BUILDS TAKE A BUILD SLOT: Sessions run in parallel \
    worktrees, so N simultaneous full builds are N compilers resident on ONE machine, which \
    OOM-kills the host. Prefix a heavy build with `lucidos build-slot -- ` (e.g. `lucidos \
    build-slot -- cargo test --release`). It waits for a free slot, then runs your command niced, \
    on a share of the cores, passing output and exit code through. Do NOT wrap cheap work such as \
    a type-check: that holds a slot for minutes to save seconds. Nothing to release: the slot \
    frees when your command exits, or if the kernel kills it.";

const BACKGROUND_PROCESS_RULE: &str = "BACKGROUND PROCESSES DON'T SURVIVE A TURN: When your turn \
    ends (you go idle), the Lucidos engine terminates you and every process you started, \
    whatever process group it runs in. A detached job (`&`, `nohup`) is therefore KILLED \
    the instant you end the turn, and nothing re-invokes you when it finishes. Under \
    Claude Code, Lucidos switches background mode OFF: the `Bash` and `Agent` tools have no \
    `run_in_background` parameter, passing one fails, and a command past its timeout is \
    killed rather than moved to the background. KEEP EVERY BLOCKING CALL INSIDE YOUR PROMPT \
    CACHE: your SESSION CONTEXT says how long it lives, and a longer gap re-bills your \
    context. Three shapes. \
    FOREGROUND, for what surely finishes in about 3 minutes: set `timeout: 240000` EXPLICITLY \
    (the 120000 ms DEFAULT cuts a build at 2 minutes). Overrunning it kills the command. POLLED BACKGROUND TASK, for longer work: `lucidos background-task \
    run --poll --description \"<what it is>\" -- '<cmd>'` runs it in your worktree. Then \
    `lucidos background-task wait <id>` (foreground, `timeout: 240000`) returns within 230 s \
    with the new output; re-issue it until finished. If your turn ends first, its finish \
    re-opens the thread. WATCHED BACKGROUND TASK, for an open-ended \
    wait such as a held lock: the same `run` without `--poll` arms an event wait. Name the work in the \
    user's words (\"the full e2e suite\"): the waiting row shows it. On `watched`, say what you \
    wait for and END YOUR TURN: the thread re-opens with the exit status and output tail. On \
    `unwatched` nothing will wake you: stop it and poll instead. REDIRECT \
    a chatty command's output to a log file \
    (`<cmd> > \"${TMPDIR:-/tmp}/$(basename \"$PWD\").log\" 2>&1`) and `tail` it, so it \
    never floods your context. SUBAGENTS ARE \
    BACKGROUND WORK TOO: a subagent dies with your process group, so its report must return \
    inside the turn. Under Claude Code every `Agent` call blocks, so it counts against the cap. \
    Put a fan-out in a single assistant message: its agents run in parallel, and it lasts as long \
    as its slowest agent. Longer delegated work goes to a child thread (`lucidos spawn-thread \
    --relation child`): END YOUR TURN, and the engine keeps a Claude Code cache warm. NEVER \
    improvise a stall instead: a filler subagent, a sleep loop, or a fabricated question (see \
    ASKING USERS).";

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
     QUIET OPTIONS: start a label with `[quiet]` when that pick needs no work from you \
     (`[quiet] Not now`), never when it promises some. The tag is hidden, and the pick ends \
     your turn. Write `[quiet: <your reply>]` to show that reply as your final answer.\n\n\
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

const APPLY_CONFIRMATION_NOTE: &str = "APPLYING YOUR WORK: An open question BLOCKS the Apply \
     button, and the user can't judge a visual change until they Apply. So when your change is \
     ready, finish the turn and the engine proposes it. Its Apply button sits in this thread. \
     Never tell the user where to apply it (no \"from the Changes panel\").";

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

const PERMISSION_CONFIG_RULE: &str = "\n\n\
    PERMISSION CONFIG: Lucidos passes `--allowedTools` to your Claude Code subprocess. This flag \
    OVERRIDES `~/.claude/settings.json` permission rules, so a tool added to `permissions.allow` \
    there has NO effect and the user keeps seeing the prompt. The prompt card remembers a grant \
    three ways: (1) `Always allow Tool(scope)` (narrow) and (2) `Always allow` (broad) record a \
    standing grant. It is PER WORKSPACE: a grant binds here and is asked again in another \
    workspace (ADR 0095). The owner edits standing grants in Settings → Permissions → Coding \
    agent permissions (ADR 0677). A session gets them at spawn and keeps that frozen flag. Bare \
    `Edit`/`Write`/`NotebookEdit` cannot be persisted through the broad button: CC settles them \
    itself, so a bare `Edit` line does nothing, and the UI hides that button. (3) `Allow <scope> \
    for this thread` records a grant this thread holds, for every tool and path, including \
    CC-protected ones. It survives a restart.";

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
    for file layout and where app data lives read `system-knowhow/best-practices`. A large \
    doc lists its parts in a `## Parts` table: read the ones your task needs \
    (`system-knowhow/js-sdk/<part>`). Load the relevant knowhow before writing app \
    code rather than guessing.";

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
    QUIET OPTIONS: pass `{label, quiet: true}` when that pick needs no work from you \
    (\"Not now\"), never when it promises some. Picking it ends your turn. Add \
    `quiet_reply` to show that text as your final answer. \
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

const NOTES_REACH_USER_SENTENCE: &str = notes_reach_user_sentence!();

const NOTES_HIDDEN_SENTENCE: &str = "Text you write in a message that also calls a tool may \
    NEVER reach the user, not even a summary of it, though your own context holds it. A message \
    that calls no tool is shown in full. Put what they must read there, or on the question card, \
    once: never write it before a tool call and again after.";

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

pub(crate) fn with_notes_visibility(
    prompt: String,
    notes: crate::runtime::claude_code_notes::NotesVisibility,
) -> String {
    use crate::runtime::claude_code_notes::NotesVisibility;
    match notes {
        NotesVisibility::Shown => prompt,
        NotesVisibility::Hidden => prompt.replace(NOTES_REACH_USER_SENTENCE, NOTES_HIDDEN_SENTENCE),
    }
}

const HTML_ARTIFACT_TYPE_SCALE_RULE: &str = "\n\n\
    HTML ARTIFACT TYPE SCALE: Size an HTML file under `artifacts/` like the chat: body \
    `0.75rem`, labels `0.8125rem`; `1rem` is a heading. Use `rem` and never size the root, \
    since the preview applies the UI scale. Full scale: `lucidos knowhow read \
    system-knowhow/best-practices/workspace-layout`, § Standalone HTML.";

/// INVARIANT: Sibling of [`REASONING_NOT_VISIBLE_RULE`], riding the same
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

/// INVARIANT: How a coding agent links a line it cites, so a click opens the file
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

/// INVARIANT: Backend-INDEPENDENT teaching appended to every coding-agent prompt by
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

/// INVARIANT: One-line mirror of the chat agent's
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

const PERMISSION_ASK_RULE: &str = "\n\n\
    SAY WHAT A COMMAND DOES AND WHY: A permission card shows a command's description as its \
    question (Codex: your escalation justification). Write it for the user: what it does and \
    why, like \"Read the triggers doc, to see how triggers install\", not \"Run sed\".";

const READ_DECISION_RULE: &str = "\n\n\
    READ DECISION: End every turn asking no question: run `lucidos request-read yes|no` \
    as a prose-free tool call, then answer once. Yes for an answer and no change (investigation, \
    review), or one read later. Else no. \
    A SUB-THREAD READ DECISION in your session context replaces this rule.";

const READ_NARROWLY_RULE: &str = "\n\n\
    READ NARROWLY: Locate before you read. Find the code with Grep or `rg -n` first, adding \
    `-l` or `-c` when only the location matters. Then read about 100 lines around the hit. \
    Read a whole file only when you need all of it. Do not re-read lines \
    already in your context unless the file changed or you are about to edit them.";

const THREAD_VERDICT_RULE: &str = "\n\n\
    THREAD VERDICTS: Call another thread stalled, stuck, failed or done only from its \
    liveness, read in THIS turn: the `# thread:` line `lucidos events query --thread-id` \
    prints on stderr. Say the state it reports. Your own earlier message is history.";

const SUCCESSOR_SESSION_RULE: &str = "\n\n\
    SUCCESSOR SESSIONS: When the user says \"handoff this session\", commit your work, write a \
    handoff narrative and run `lucidos successor start --narrative <file>` (see --help). In a \
    Lucidos thread, advice to run `/clear` or write a handoff file means the same. A HANDOFF \
    CHECKPOINT is a `lucidos successor offer --at <name>` step in a repo flow: run it and follow \
    it. Setup: `lucidos knowhow read system-knowhow/coding-agent-flows`.";

const CHECKPOINT_AND_PAUSE_RULE: &str = "\n\n\
    CHECKPOINT AND PAUSE: When told to checkpoint and pause, commit, run `lucidos threads \
    hold --reason \"<why>\"`, then end your turn. Nothing runs there or below until the owner \
    resumes it.";

const WAKE_DECISION_RULE: &str = "\n\n\
    WAKE DECISION: You are a sub-thread, and your turn's final message reports to your parent \
    thread. Before that message, run `lucidos wake-parent yes|no`. Yes when your parent must \
    act on it now: it waits for this result to take a next step, combine results, retry or \
    decide. No when it only informs: the report is filed, and your parent reads it on its next \
    turn. A failure always wakes it. Unsure, skip it: Lucidos decides.";

pub(super) async fn sub_thread_section(pool: &sqlx::PgPool, thread_id: uuid::Uuid) -> String {
    sub_thread_rules(
        crate::engine::turn_end::wake_decision::WakeDuty::of_thread(pool, thread_id).await,
    )
}

fn sub_thread_rules(duty: crate::engine::turn_end::wake_decision::WakeDuty) -> String {
    use crate::engine::turn_end::wake_decision::WakeDuty;
    let wake_rule = if duty == WakeDuty::Decide {
        WAKE_DECISION_RULE
    } else {
        ""
    };
    match duty.read_rule() {
        "" => wake_rule.to_string(),
        read_rule => format!("{wake_rule}\n\n{read_rule}"),
    }
}

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
        log_legacy!("[AgentSession] parent title for {}: {}", thread_id, e);
        None
    })
    .flatten();
    integrated_rule(parent_title.as_deref())
}

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
         branch, and your parent's agent merges it. For any ordinary decision, including plan \
         approval, run `lucidos wake-parent yes` and end your turn with the decision and options \
         in your report. Your parent's agent can reply by messaging this thread. If a new plan \
         needs implementation approval, commit it and include its path in the report. The parent can bring \
         that revision into its branch and obtain the owner's implementation approval there. \
         Then rerun `lucidos planned mark --plan <path>` here to inherit the approval. Use the question \
         tool only for a prepared permission request that needs the owner's authority."
    )
}

const NO_STANDING_INSTRUCTION_NOTE: &str = "\n\n\
    THE OWNER DID NOT OPEN THIS TURN: another thread or an event did. So Lucidos refuses, with a \
    403, an act on a thread outside your own subtree, such as an Apply, archive or cancel. \
    Spawning a thread, top-level or child, needs no authority. An ordinary question card cannot \
    authorize such an act, even when the owner picks the option naming it. When the decision \
    you would ask about is one, ask for a permission INSTEAD of an ordinary card. Run \
    `lucidos ask-permission <action> --thread <uuid> --reason \"<why>\"` \
    (`lucidos ask-permission --help` lists the actions). Then ask your question tool with the \
    id it prints as the only question. Lucidos shows its own permission card in its place. A \
    409 from it means you hold the authority after all, for example because the owner has \
    written to you since or already allowed it. Then go ahead as usual.";

pub(super) async fn standing_instruction_section(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> &'static str {
    if crate::engine::standing_instruction::carries_standing_instruction(
        pool,
        Some(thread_id),
        None,
    )
    .await
    {
        ""
    } else {
        NO_STANDING_INSTRUCTION_NOTE
    }
}

const SESSION_CONTEXT_HEADER: &str = "SESSION CONTEXT: Lucidos sends this once, at the start of \
    your session. It holds what differs from one session to the next. Your system prompt holds \
    the rest.";

const SESSION_CONTEXT_END: &str = "---\nEnd of session context.";

#[derive(Debug, Default)]
pub(super) struct SessionContext {
    sections: Vec<String>,
    images: Vec<crate::engine::types::ChatImage>,
}

impl SessionContext {
    pub(super) fn push(&mut self, section: &str) {
        let section = section.trim();
        if !section.is_empty() {
            self.sections.push(section.to_string());
        }
    }

    pub(super) fn attach_images(&mut self, images: Vec<crate::engine::types::ChatImage>) {
        self.images.extend(images);
    }

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

pub(super) fn prompt_cache_section(
    repo_ttl: CacheTtl,
    coding_agent: crate::runtime::CodingAgent,
) -> String {
    let lifetime = match coding_agent {
        crate::runtime::CodingAgent::ClaudeCode => repo_ttl.lifetime(),
        crate::runtime::CodingAgent::Codex => None,
    };
    match lifetime {
        Some(lifetime) => format!(
            "PROMPT CACHE: Your prompt cache lives {}. Keep every blocking call, subagents \
             included, under {}.",
            lifetime.lifetime_words(),
            lifetime.blocking_cap_words()
        ),
        None => format!(
            "PROMPT CACHE: Lucidos does not know how long your prompt cache lives. Keep every \
             blocking call, subagents included, under {}: that fits the shortest lifetime a \
             provider documents.",
            CacheLifetime::FiveMinutes.blocking_cap_words()
        ),
    }
}

const BRANCH_SECTION_LEAD: &str = "BRANCH: Your worktree is on branch";

fn branch_section(branch: &str, base_ref: Option<&str>) -> String {
    match base_ref {
        Some(base) => format!("{BRANCH_SECTION_LEAD} `{branch}`, based on {base}."),
        None => format!("{BRANCH_SECTION_LEAD} `{branch}`."),
    }
}

pub(super) struct SpawnPromptInputs<'a> {
    pub(super) flavor: String,
    pub(super) coding_agent: crate::runtime::CodingAgent,
    pub(super) workspace_sections: &'a [&'a str],
    pub(super) branch: &'a str,
    pub(super) base_ref: Option<&'a str>,
    pub(super) repo_id: Option<&'a str>,
    pub(super) app_id: Option<&'a str>,
    pub(super) sub_thread_section: &'a str,
    pub(super) cache_ttl: CacheTtl,
}

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
    context.push(inputs.sub_thread_section);
    context.push(&prompt_cache_section(inputs.cache_ttl, inputs.coding_agent));
    (system_prompt, context)
}

/// INVARIANT: Append backend-specific rules — plus the backend-independent
/// [`REASONING_NOT_VISIBLE_RULE`], which rides every prompt here — to a finished
/// system prompt — the single
/// point where the two coding-agent backends diverge. Claude Code prompts gain
/// [`PERMISSION_CONFIG_RULE`] (the `--allowedTools` / standing grants
/// mechanics are CC-only); Codex prompts replace [`ASK_USER_QUESTION_RULE`]
/// with [`CODEX_ASK_USER_QUESTION_RULE`] and append [`codex_exec_question_rule`]
/// and [`CODEX_CLI_RULE`]. Each
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
    let prompt = format!(
        "{prompt}{REASONING_NOT_VISIBLE_RULE}{SHOWING_AN_IMAGE_RULE}\
         {HTML_ARTIFACT_TYPE_SCALE_RULE}{NAMES_NOT_IDS_RULE}{NO_IMPERSONATION_RULE}\
         {PERMISSION_ASK_RULE}{READ_DECISION_RULE}{READ_NARROWLY_RULE}{THREAD_VERDICT_RULE}\
         {SUCCESSOR_SESSION_RULE}{CHECKPOINT_AND_PAUSE_RULE}"
    );
    match coding_agent {
        crate::runtime::CodingAgent::ClaudeCode => format!("{prompt}{PERMISSION_CONFIG_RULE}"),
        crate::runtime::CodingAgent::Codex => {
            let prompt = prompt.replace(ASK_USER_QUESTION_RULE, CODEX_ASK_USER_QUESTION_RULE);
            let exec_question_rule = codex_exec_question_rule();
            format!("{prompt}{exec_question_rule}{CODEX_CLI_RULE}{CODEX_SLASH_COMMANDS_RULE}")
        }
    }
}

fn codex_exec_question_rule() -> String {
    let wait_ms = crate::engine::cc_settings::USER_ANSWER_WAIT_SECS * 1000;
    format!(
        "\n\nASKING FROM EXEC: when an `exec` cell calls `ask_user_question`, make its first line \
         `// @exec: {{\"yield_time_ms\": {wait_ms}}}`, so it blocks until the user answers. \
         Never poll a question cell with short `wait` calls: each one is a billed model turn."
    )
}

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
         Your cwd is the app folder at `data/apps/{app_id}/`. The worktree root is three \
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::merge_prompts::{
        build_lane_catch_up_prompt, build_lane_thread_prompt, build_merge_prompt, LaneMemberBrief,
    };

    #[test]
    fn the_claude_code_rule_names_the_quiet_label_prefix() {
        let prefix = crate::engine::question_parsing::QUIET_LABEL_PREFIX;
        assert!(ASK_USER_QUESTION_RULE.contains(&format!("`{prefix}`")));
        assert!(ASK_USER_QUESTION_RULE.contains("[quiet: <your reply>]"));
        assert!(CODEX_ASK_USER_QUESTION_RULE.contains("quiet: true"));
        assert!(CODEX_ASK_USER_QUESTION_RULE.contains("quiet_reply"));
    }

    #[test]
    fn only_codex_learns_to_hold_a_question_cell_open() {
        let wait_ms = crate::engine::cc_settings::USER_ANSWER_WAIT_SECS * 1000;
        let pragma = format!("// @exec: {{\"yield_time_ms\": {wait_ms}}}");
        let codex = append_backend_rules(String::new(), crate::runtime::CodingAgent::Codex);
        let claude = append_backend_rules(String::new(), crate::runtime::CodingAgent::ClaudeCode);
        assert!(codex.contains(&pragma), "{codex}");
        assert!(!claude.contains("@exec"), "{claude}");
    }

    #[test]
    fn the_read_decision_command_is_allowed_without_a_card() {
        assert!(READ_DECISION_RULE.contains("`lucidos request-read yes|no`"));
        assert!(crate::engine::claude_code::ENGINE_ALLOWED_TOOLS
            .contains(&"Bash(lucidos request-read:*)"));
    }

    #[test]
    fn the_hardening_rule_exempts_by_the_engine_plan_file_predicate() {
        let _: fn(&str) -> bool = crate::engine::git_ops::is_plan_file;
        assert!(HARDENING_RULE.contains("(`git_ops::is_plan_file`)"));
        assert!(HARDENING_RULE.contains("skip `/harden` there."));
    }

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

    #[test]
    fn a_sub_thread_is_told_its_read_rule_by_wake_duty() {
        use crate::engine::thread_events::WakeMode;
        use crate::engine::turn_end::wake_decision::WakeDuty;

        for mode in [WakeMode::Wake, WakeMode::WakeWhenAllDone] {
            let section = sub_thread_rules(WakeDuty::Declared(mode));
            assert!(
                section.contains("SUB-THREAD READ DECISION: "),
                "{mode:?}: {section}"
            );
            assert!(!section.contains("WAKE DECISION"), "{mode:?}: {section}");
        }

        let decide = sub_thread_rules(WakeDuty::Decide);
        assert!(decide.starts_with(WAKE_DECISION_RULE), "{decide}");
        assert!(
            decide.contains("SUB-THREAD READ DECISION, only when you wake your parent: "),
            "{decide}"
        );

        assert_eq!(sub_thread_rules(WakeDuty::Declared(WakeMode::NoWake)), "");
        assert_eq!(sub_thread_rules(WakeDuty::NoParent), "");
    }

    #[test]
    fn the_sub_thread_read_rule_keeps_the_must_see_criteria() {
        use crate::engine::thread_events::WakeMode;
        use crate::engine::turn_end::wake_decision::WakeDuty;
        let rule = WakeDuty::Declared(WakeMode::Wake).read_rule();
        for needle in [
            "A routine result (a pass, nothing found, already fixed, a ready change, a status, \
             a review verdict) \
             is a no",
            "a failure, a gap the user must know about, or something only the user can do or \
             decide, such as a payment, a sign-in or an approval",
            "Your parent asks the user for any approval.",
            "A turn the user opens in this thread is answered as usual.",
        ] {
            assert!(rule.contains(needle), "missing {needle:?}");
        }
    }

    #[test]
    fn the_generic_read_rule_defers_to_the_sub_thread_context() {
        assert!(READ_DECISION_RULE
            .contains("A SUB-THREAD READ DECISION in your session context replaces this rule."));
        assert!(!WAKE_DECISION_RULE.contains("SUB-THREAD"));
    }

    #[tokio::test]
    async fn the_section_follows_the_wake_mode_the_spawn_declared() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{
            ActorMode, EventChannel, EventMeta, ThreadEvent, WakeMode,
        };
        use crate::test_support::{setup_test_db, teardown_test_db};
        use uuid::Uuid;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = Uuid::new_v4();
        let spawn = |thread_id: Uuid, parent: Option<Uuid>, wake_mode: Option<WakeMode>| {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::MessageReceived {
                    provider: None,
                    voice_session_id: None,
                    wake_mode,
                    text: "work".into(),
                    user_image_hashes: vec![],
                    device_id: None,
                    image_description: None,
                    parent_thread_id: parent,
                    spawning_event_id: None,
                    mode: ActorMode::Agent,
                    model: None,
                    reasoning_effort: None,
                    origin: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
        };
        let (woken, filed, undeclared) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        spawn(parent, None, None).await.unwrap();
        spawn(woken, Some(parent), Some(WakeMode::Wake))
            .await
            .unwrap();
        spawn(filed, Some(parent), Some(WakeMode::NoWake))
            .await
            .unwrap();
        spawn(undeclared, Some(parent), None).await.unwrap();

        assert!(sub_thread_section(&pool, woken)
            .await
            .contains("SUB-THREAD READ DECISION: "));
        assert_eq!(sub_thread_section(&pool, filed).await, "");
        assert_eq!(sub_thread_section(&pool, parent).await, "");
        assert!(sub_thread_section(&pool, undeclared)
            .await
            .contains("only when you wake your parent"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn scratch_paths_name_the_per_user_temp_dir() {
        assert!(BACKGROUND_PROCESS_RULE.contains("${TMPDIR:-/tmp}/"));
        for rule in [BACKGROUND_PROCESS_RULE, SHOWING_AN_IMAGE_RULE] {
            let rest = rule.replace("${TMPDIR:-/tmp}/", "");
            assert!(!rest.contains("/tmp/"), "a bare /tmp scratch path: {rule}");
        }
    }

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

    #[test]
    fn two_sessions_in_one_workspace_share_the_appended_prompt() {
        let workspace_sections = ["\n\nLITERACY", "\n\nMEMORY", "\n\nWIDGETS"];
        for (flavor, base) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let session = |branch, base_ref, sub_thread_section, cache_ttl| {
                    spawn_prompts(SpawnPromptInputs {
                        flavor: base.clone(),
                        coding_agent: agent,
                        workspace_sections: &workspace_sections,
                        branch,
                        base_ref,
                        repo_id: Some("3f9c1b2e"),
                        app_id: None,
                        sub_thread_section,
                        cache_ttl,
                    })
                };
                let (one, one_context) = session("lucidos-one", None, "", CacheTtl::Unknown);
                let (two, two_context) = session(
                    "lucidos-two",
                    Some("origin/release"),
                    WAKE_DECISION_RULE,
                    CacheTtl::Reported(CacheLifetime::OneHour),
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

    #[test]
    fn the_background_rule_names_no_cache_lifetime() {
        for lifetime in CacheLifetime::ALL {
            for words in [lifetime.lifetime_words(), lifetime.blocking_cap_words()] {
                assert!(!BACKGROUND_PROCESS_RULE.contains(words), "{words}");
            }
        }
    }

    #[test]
    fn the_cache_section_states_a_known_lifetime() {
        use crate::runtime::CodingAgent::ClaudeCode;
        for lifetime in CacheLifetime::ALL {
            for ttl in [CacheTtl::Reported(lifetime), CacheTtl::Requested(lifetime)] {
                let section = prompt_cache_section(ttl, ClaudeCode);
                assert!(
                    section.contains(&format!("lives {}.", lifetime.lifetime_words())),
                    "{ttl:?}: {section}"
                );
                assert!(
                    section.contains(&format!("under {}.", lifetime.blocking_cap_words())),
                    "{ttl:?}: {section}"
                );
            }
        }
    }

    #[test]
    fn the_cache_section_states_an_unknown_lifetime() {
        use crate::runtime::CodingAgent::{ClaudeCode, Codex};
        let one_hour = CacheTtl::Reported(CacheLifetime::OneHour);
        for (ttl, agent) in [
            (CacheTtl::Unknown, ClaudeCode),
            (CacheTtl::Unknown, Codex),
            (one_hour, Codex),
        ] {
            let section = prompt_cache_section(ttl, agent);
            assert_eq!(
                section,
                "PROMPT CACHE: Lucidos does not know how long your prompt cache lives. Keep \
                 every blocking call, subagents included, under 4 minutes: that fits the \
                 shortest lifetime a provider documents.",
                "{ttl:?}/{agent:?}"
            );
        }
    }

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

    #[test]
    fn lane_system_prompt_keeps_the_landing_merges_and_asks_for_judgment() {
        let prompt = lane_thread_system_prompt();
        assert!(prompt.contains("question tool"));
        assert!(prompt.contains("Never rebase"));
        assert!(prompt.contains("/harden"));
        assert!(prompt.contains("landing merge"));
    }

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

    const LUCIDOS_ONLY_TOKENS: &[&str] = &[
        "/harden",
        "/cpa",
        "./scripts/stop.sh",
        "./scripts/web-dev.sh",
        "./scripts/e2e",
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
        assert!(
            prompt.contains("lucidos knowhow read system-knowhow/building-an-app"),
            "must point app sessions at building-an-app knowhow via the CLI",
        );
        assert!(
            prompt.contains("lucidos knowhow list"),
            "must tell app sessions the full knowhow catalog is available via the CLI",
        );
        for needle in [
            "`system-knowhow/js-sdk`",
            "`## Parts` table",
            "system-knowhow/js-sdk/<part>",
        ] {
            assert!(
                prompt.contains(needle),
                "app sessions must learn to read a split doc's parts (`{needle}`)",
            );
        }
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
        assert_no_lucidos_only_tokens(
            &external_repo_recovery_system_prompt("Acme"),
            "external_repo_recovery_system_prompt (with restart note)",
        );
    }

    #[test]
    fn external_prompts_keep_pkill_prevention() {
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
                "FOREGROUND",
                "`timeout: 240000`",
                "KEEP EVERY BLOCKING CALL INSIDE YOUR PROMPT CACHE",
                "SESSION CONTEXT says how long it lives",
                "lucidos background-task run --poll",
                "lucidos background-task wait <id>",
                "WATCHED BACKGROUND TASK",
                "END YOUR TURN",
                "`unwatched`",
                "REDIRECT a chatty command's output to a log file",
                "background mode OFF",
                "killed rather than moved to the background",
                "SUBAGENTS ARE BACKGROUND WORK TOO",
                "every `Agent` call blocks",
                "single assistant message",
                "NEVER improvise a stall",
                "fabricated question",
                "counts against the cap",
                "slowest agent",
                "`lucidos spawn-thread --relation child`",
            ] {
                assert!(
                    prompt.contains(needle),
                    "{label} must warn that background processes die at turn end (`{needle}`)",
                );
            }
            for banned in ["run_in_background: false", "run_in_background: true"] {
                assert!(
                    !prompt.contains(banned),
                    "{label} must not ask for `{banned}`: the parameter no longer exists",
                );
            }
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
            assert!(
                full.contains("SUBAGENTS ARE BACKGROUND WORK TOO"),
                "worktree_system_prompt must keep the subagent arm for {:?}",
                agent,
            );
        }

        assert!(
            !conflict_resolution_system_prompt()
                .contains("BACKGROUND PROCESSES DON'T SURVIVE A TURN"),
            "merge-conflict prompt should omit the background-process rule",
        );
    }

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
            "Add `--relation child` only to act on its result",
        ] {
            assert!(CODEX_CLI_RULE.contains(needle), "missing {needle:?}");
        }
    }

    const PROMPT_FLAVOR_CEILINGS: &[(&str, &str, usize)] = &[
        ("worktree", "claude-code", 26187),
        ("worktree", "codex", 26371),
        ("external_repo", "claude-code", 19000),
        ("external_repo", "codex", 19185),
        ("recovery", "claude-code", 24904),
        ("recovery", "codex", 25088),
        ("external_repo_recovery", "claude-code", 18902),
        ("external_repo_recovery", "codex", 19087),
        ("app_worktree", "claude-code", 21563),
        ("app_worktree", "codex", 21792),
        ("app_worktree_recovery", "claude-code", 20123),
        ("app_worktree_recovery", "codex", 20352),
        ("conflict_resolution", "claude-code", 7691),
        ("conflict_resolution", "codex", 10018),
    ];

    fn all_backends() -> [(crate::runtime::CodingAgent, &'static str); 2] {
        [
            (crate::runtime::CodingAgent::ClaudeCode, "claude-code"),
            (crate::runtime::CodingAgent::Codex, "codex"),
        ]
    }

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

    #[test]
    fn every_session_reads_liveness_before_a_thread_verdict() {
        for (flavor, prompt) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let prompt = append_backend_rules(prompt.clone(), agent);
                assert!(
                    prompt.contains(THREAD_VERDICT_RULE),
                    "{flavor}/{label} lacks the thread verdict rule"
                );
            }
        }
        assert!(THREAD_VERDICT_RULE.contains("read in THIS turn"));
        assert!(THREAD_VERDICT_RULE.contains("earlier message is history"));
        assert!(THREAD_VERDICT_RULE.contains("# thread:"));
    }

    #[test]
    fn every_session_is_taught_the_successor_session() {
        for (flavor, prompt) in all_prompt_flavors() {
            for (agent, label) in all_backends() {
                let prompt = append_backend_rules(prompt.clone(), agent);
                assert!(
                    prompt.contains(SUCCESSOR_SESSION_RULE),
                    "{flavor}/{label} lacks the successor rule"
                );
            }
        }
        assert!(SUCCESSOR_SESSION_RULE.contains("lucidos successor start"));
        assert!(SUCCESSOR_SESSION_RULE.contains("`/clear`"));
        assert!(SUCCESSOR_SESSION_RULE.contains("HANDOFF CHECKPOINT"));
        assert!(SUCCESSOR_SESSION_RULE.contains("`lucidos successor offer --at <name>`"));
        assert!(SUCCESSOR_SESSION_RULE.contains("system-knowhow/coding-agent-flows"));
        let request = crate::engine::successor::HANDOFF_REQUEST;
        assert!(SUCCESSOR_SESSION_RULE.contains(&format!("\"{request}\"")));
    }

    #[test]
    fn only_external_repo_sessions_are_taught_the_build_slot_wrapper() {
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
                    for needle in [
                        "does NOT discharge this",
                        "your authority stops at the product's users",
                        "NOT by whether it fires on this machine or workspace",
                        "A finding you DO fix in this change needs no question",
                    ] {
                        assert!(
                            full.contains(needle),
                            "{label}/{} must keep the raise-findings bar (missing: {needle:?})",
                            agent.as_str()
                        );
                    }
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
                assert!(
                    !full.contains("just before a tool call"),
                    "{label} ({agent:?}) still carries the hidden-notes stopgap",
                );
            }
        }
        let codex = append_backend_rules(
            worktree_system_prompt("dev"),
            crate::runtime::CodingAgent::Codex,
        );
        assert!(
            codex.contains("ask_user_question` tool (on the `lucidos` MCP"),
            "Codex prompt must still swap in the MCP ask_user_question rule",
        );
    }

    #[test]
    fn the_read_decision_comes_before_the_answer_in_every_flavor() {
        for (label, base) in all_prompt_flavors() {
            for (backend, backend_label) in all_backends() {
                let built = append_backend_rules(base.clone(), backend);
                assert!(
                    built.contains("as a prose-free tool call, then answer once"),
                    "{label}/{backend_label}"
                );
            }
        }
    }

    #[test]
    fn a_hidden_notes_session_reads_the_hard_rule_in_place_of_the_note_sentence() {
        use crate::runtime::claude_code_notes::NotesVisibility::{Hidden, Shown};
        for (label, base) in all_prompt_flavors() {
            let built = append_backend_rules(base, crate::runtime::CodingAgent::ClaudeCode);
            let hidden = with_notes_visibility(built.clone(), Hidden);
            assert!(hidden.contains(NOTES_HIDDEN_SENTENCE), "{label}");
            assert!(hidden.contains("NEVER reach the user"), "{label}");
            assert!(hidden.contains("never write it before a tool call and again after"));
            assert!(
                !hidden.contains(NOTES_REACH_USER_SENTENCE),
                "{label}: both rules in one prompt"
            );
            assert!(
                !hidden.contains("only as a short note"),
                "{label}: a hidden session is never told the note arrives"
            );
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
                    "`lucidos knowhow read system-knowhow/best-practices/workspace-layout`, \
                     § Standalone HTML",
                ] {
                    assert!(
                        full.contains(needle),
                        "{label} ({agent:?}) must carry the HTML artifact type scale (`{needle}`)",
                    );
                }
            }
        }
    }

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
                    "lucidos data write artifacts/",
                    "![what it shows](artifacts/x.png)",
                    "their step records that you read a file",
                    "only stores it, and the user still sees nothing",
                    "`preview` if your tool has one",
                    "Before a card, put it ON the question card",
                    "arrive as a short summary that drops it",
                    "in the reply ending your turn",
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
                assert!(
                    !full.contains("without that line in the same message"),
                    "{label} ({agent:?}) must not send a picture into the words before a card",
                );
            }
        }
    }

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

    #[test]
    fn codex_prompts_map_slash_commands_to_playbook_files() {
        let flavors: &[(&str, String)] = &[
            ("worktree", worktree_system_prompt("dev")),
            ("recovery", recovery_system_prompt("dev")),
            (
                "conflict_resolution",
                conflict_resolution_system_prompt().to_string(),
            ),
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
            for needle in ["APPROVE A PLAN OR AN", "`Approve` and `Request changes`"] {
                assert!(
                    prompt.contains(needle),
                    "{label} must carve plan approval OUT of the no-confirmations rule \
                     and name its option pair (missing: {needle:?})",
                );
            }
            assert!(
                prompt.contains("make that fork the second option"),
                "{label} must state that the approval option pair is a FLOOR, not a fixed \
                 shape, so a real fork replaces `Request changes` instead of joining it",
            );
            assert!(
                prompt.contains("Tapping a fork is still an approval"),
                "{label} must say a fork answer approves that variant rather than rejecting \
                 the plan (without naming the plan marker, which is Lucidos-source only)",
            );
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
            assert!(
                prompt.contains("None of these") && prompt.contains("still welcome"),
                "{label} must keep a meaningful opt-out option legal",
            );
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
                "ASK FOR APPROVAL WITH THE QUESTION TOOL",
                "`Approve` and `Request changes`",
                "make that fork the second option",
                "A tapped fork is an approval too",
                "the card's question carries a short markdown summary",
                "A typed reply that asks, suggests or questions is a request for changes",
                "Offer the options the skill's approval checkpoint prints",
            ] {
                assert!(
                    base.contains(needle),
                    "{label} must carry the implementation-plan rule (`{needle}`)",
                );
            }
            assert!(
                base.contains("ASKING USERS:"),
                "{label} names \"the ASKING USERS section\" for the approval tool, so \
                 that section must be in the same prompt",
            );
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

    #[test]
    fn plan_approval_is_carved_out_of_the_no_confirmations_rule_on_both_backends() {
        let base = worktree_system_prompt("dev");
        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        let codex = append_backend_rules(base, crate::runtime::CodingAgent::Codex);

        for (label, prompt) in [("claude-code", &cc), ("codex", &codex)] {
            assert!(
                prompt.contains("`Approve` and `Request changes`"),
                "{label} must name the same approval option pair as the skill and the \
                 cc-plan-gate deny message",
            );
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

    #[test]
    fn lucidos_prompts_carry_full_apply_restart_rule() {
        let cases: &[(&str, String)] = &[
            ("worktree_system_prompt", worktree_system_prompt("dev")),
            ("recovery_system_prompt", recovery_system_prompt("dev")),
        ];
        let required_file_types = [
            "`.rs`",
            "`Cargo.toml`",
            "`Cargo.lock`",
            "`.sql` migration under `migrations/`",
            "`packages/lucidos-sdk/`",
            "`crates/lucidos-agents-api/src/api/sdk_iframe.css`",
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

    #[test]
    fn codex_prompts_carry_cli_and_question_teaching() {
        let base = worktree_system_prompt("dev");
        let codex = append_backend_rules(base.clone(), crate::runtime::CodingAgent::Codex);
        for needle in [
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

        let cc = append_backend_rules(base.clone(), crate::runtime::CodingAgent::ClaudeCode);
        assert!(
            cc.starts_with(&base),
            "backend rules must append, not replace, the worktree prompt",
        );
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

    #[test]
    fn permission_config_rule_is_claude_code_only() {
        let base = worktree_system_prompt("dev");
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
            "Settings → Permissions → Coding agent permissions",
            "PER WORKSPACE",
        ] {
            assert!(
                cc.contains(needle),
                "Claude Code prompt must carry the full permission-config rule (`{needle}`)",
            );
        }

        assert!(!cc.contains("cc-allowed-tools"));

        let codex = append_backend_rules(base, crate::runtime::CodingAgent::Codex);
        for needle in ["PERMISSION CONFIG:", "--allowedTools", "cc-allowed-tools"] {
            assert!(
                !codex.contains(needle),
                "Codex prompt must NOT carry the CC-only permission-config rule (`{needle}`) — \
                 Codex permissions surface via its sandbox + approval-policy model",
            );
        }
    }

    #[test]
    fn conflict_resolution_prompt_requires_user_facing_summary() {
        let prompt = conflict_resolution_system_prompt();
        let prompt_lower = prompt.to_lowercase();
        assert!(
            prompt_lower.contains("summar") || prompt_lower.contains("explain what"),
            "conflict_resolution_system_prompt must instruct CC to summarize what it \
             resolved as its final assistant message — otherwise terse CC turns \
             ('done.', empty text) leave the user with no closure: the recovery \
             thread sits at Idle with no visible signal that the merge succeeded"
        );
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

    #[test]
    fn the_apply_restart_rule_examples_agree_with_files_require_restart() {
        use crate::engine::git_ops::files_require_restart;
        let triggers = [
            (
                "a non-test `.rs` file",
                "crates/lucidos-agents-api/src/lib.rs",
            ),
            ("`Cargo.toml`", "Cargo.toml"),
            ("`Cargo.lock`", "Cargo.lock"),
            (
                "a `.sql` migration under `migrations/`",
                "crates/lucidos-core/migrations/001.sql",
            ),
            (
                "`packages/lucidos-sdk/`",
                "packages/lucidos-sdk/src/index.ts",
            ),
            (
                "`crates/lucidos-engine/`",
                "crates/lucidos-engine/assets/x.txt",
            ),
            (
                "`crates/lucidos-gateway/`",
                "crates/lucidos-gateway/assets/x.txt",
            ),
            ("`crates/lucidos-cli/`", "crates/lucidos-cli/assets/x.txt"),
            (
                "`crates/lucidos-agents-api/src/api/sdk_iframe.css`",
                "crates/lucidos-agents-api/src/api/sdk_iframe.css",
            ),
            (
                "`sdk_iframe_audio.js`",
                "crates/lucidos-agents-api/src/api/sdk_iframe_audio.js",
            ),
            (
                "`crates/lucidos-app/src/styles/global/shared-components.css`",
                "crates/lucidos-app/src/styles/global/shared-components.css",
            ),
            ("`index.html`", "crates/lucidos-app/index.html"),
            (
                "the fonts",
                "crates/lucidos-app/src/assets/fonts/inter.woff2",
            ),
        ];
        for (snippet, file) in triggers {
            assert!(
                APPLY_RESTART_RULE.contains(snippet),
                "the rule must name `{snippet}`"
            );
            assert!(
                files_require_restart(&[file.to_string()]),
                "the rule names `{snippet}` as a restart trigger but `{file}` does not restart"
            );
        }
        assert!(APPLY_RESTART_RULE.contains("Frontend-only edits"));
        for file in [
            "crates/lucidos-app/src/components/Widget.tsx",
            "crates/lucidos-app/src/styles/widget.css",
        ] {
            assert!(
                !files_require_restart(&[file.to_string()]),
                "the rule says frontend-only edits do not restart, but `{file}` does"
            );
        }
    }

    #[test]
    fn the_harden_rule_keeps_its_last_three_sentences_for_both_backends() {
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for base in [worktree_system_prompt("dev"), recovery_system_prompt("dev")] {
                let full = append_backend_rules(base, agent);
                for needle in [
                    "If Phase 0 reports ALREADY_HARDENED, say so and stop.",
                    "Never tell the user you are postponing, deferring, or skipping it.",
                    "There is no such option in the system, and Apply runs hardening \
                     synchronously when the marker is missing, so a postponement you \
                     announce is a confusing lie.",
                ] {
                    assert!(
                        full.contains(needle),
                        "{agent:?} prompt lost a harden sentence: {needle}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_plan_rule_reaches_codex_whole_and_leaves_out_the_security_fix_lane() {
        for base in [worktree_system_prompt("dev"), recovery_system_prompt("dev")] {
            for agent in [
                crate::runtime::CodingAgent::ClaudeCode,
                crate::runtime::CodingAgent::Codex,
            ] {
                let full = append_backend_rules(base.clone(), agent);
                assert!(
                    full.contains(IMPLEMENTATION_PLAN_RULE),
                    "{agent:?} prompt must carry the whole plan rule"
                );
            }
        }
        for lane in ["--security-fix", "bounded_security_fix"] {
            assert!(
                !IMPLEMENTATION_PLAN_RULE.contains(lane),
                "the security-fix lane belongs in the gate message and the skill, not here"
            );
        }
    }

    #[test]
    fn the_ask_rules_name_no_fresh_session_option_and_keep_the_dangling_item_paragraph() {
        for rule in [ASK_USER_QUESTION_RULE, CODEX_ASK_USER_QUESTION_RULE] {
            assert!(
                !rule.contains("fresh session"),
                "external-repo sessions share this rule and have no plan marker"
            );
            assert!(rule.contains("A DANGLING ITEM IS A DECISION IN"));
        }
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            let full = append_backend_rules(external_repo_system_prompt("Acme"), agent);
            assert!(full.contains("A DANGLING ITEM IS A DECISION IN"));
            assert!(!full.contains("Implement in a fresh session"));
        }
    }

    #[test]
    fn every_shared_rule_rides_the_chokepoint_for_every_flavor_and_backend() {
        let shared: [(&str, &str); 11] = [
            ("REASONING_NOT_VISIBLE_RULE", REASONING_NOT_VISIBLE_RULE),
            ("SHOWING_AN_IMAGE_RULE", SHOWING_AN_IMAGE_RULE),
            (
                "HTML_ARTIFACT_TYPE_SCALE_RULE",
                HTML_ARTIFACT_TYPE_SCALE_RULE,
            ),
            ("NAMES_NOT_IDS_RULE", NAMES_NOT_IDS_RULE),
            ("NO_IMPERSONATION_RULE", NO_IMPERSONATION_RULE),
            ("PERMISSION_ASK_RULE", PERMISSION_ASK_RULE),
            ("READ_DECISION_RULE", READ_DECISION_RULE),
            ("READ_NARROWLY_RULE", READ_NARROWLY_RULE),
            ("THREAD_VERDICT_RULE", THREAD_VERDICT_RULE),
            ("SUCCESSOR_SESSION_RULE", SUCCESSOR_SESSION_RULE),
            ("CHECKPOINT_AND_PAUSE_RULE", CHECKPOINT_AND_PAUSE_RULE),
        ];
        for (label, base) in all_prompt_flavors() {
            for (agent, agent_label) in all_backends() {
                let full = append_backend_rules(base.clone(), agent);
                for (name, rule) in shared {
                    assert!(full.contains(rule), "{label}/{agent_label} lost {name}");
                }
            }
        }
    }
}
