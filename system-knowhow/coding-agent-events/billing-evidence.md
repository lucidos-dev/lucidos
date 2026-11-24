---
name: Coding Agent Lifecycle Events, Billing evidence
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Billing evidence

`CodingAgentBillingObserved` records passive metadata, never a turn or usage row.
Its `coding_agent` is `claude-code` or `codex`; `launch_id` scopes the runtime launch.
`attribution` has one of these shapes:

- `{status: "known", provider, source}`
- `{status: "unresolved", reason, source}`

Sources are `claude-initialize`, `codex-account` and `unavailable`.
Reasons are `missing-evidence`, `unsupported-backend`, `unknown-auth-method`,
`unknown-endpoint` and `conflicting-evidence`.
The provider IDs are `claude-gateway`, `vertex`, `bedrock`, `foundry`, `anthropic`,
`anthropic-subscription`, `openai` and `chatgpt-plan`.

Every new coding-agent `ContextCaptured` sets its existing `provider` field to
one of those IDs or `unknown`. There is no nested billing block.
Old captures without `provider` remain unattributed; do not infer from model names.
Producer, model and token counts retain their existing meanings.

`billing_mode` is `plan` for `anthropic-subscription` and `chatgpt-plan`,
`token-metered` for other known providers except `claude-gateway`, and `unknown`
for the gateway or unresolved evidence. Plan costs are list-price equivalents,
not extra charges. Gateway identity alone does not prove a billing contract.

Claude queries initialize and status on the serving process, including its wrapper,
cwd, environment and account pin. Non-gateway attribution needs complete endpoint
status. Codex app-server refreshes account, config and provider reports before
each turn, with a bounded wait. Typed `modelProvider/list` builtin OpenAI settings
resolve default routing when `config/read` agrees. Legacy servers need explicit
builtin endpoint fields; missing config alone never proves default routing.
Codex exec exposes no proven auth report and stays unresolved.

Codex app-server also reports restored usage while resuming a thread.
Each notification still records a capture with its original counts, including repeats.
A restored snapshot retains the latest prior main-session capture's provider only
when its exact usage vector and persisted Codex backend-session binding match.
The lookup stops before the current launch and cannot cross an unbound process boundary.

Missing history, unknown prior providers and mismatched usage remain `unknown`.
A late notification with no proven restored or current turn also remains `unknown`.
Restored attribution never pins billing for new work; new turns require fresh process evidence.

Opaque wrappers and unsupported protocols also stay unresolved.
This covers the macOS/Linux runtime paths; arbitrary MDM policies need actual
serving-process evidence, not host configuration guesses.

Use a running source version with this recording path and genuinely new captures
for verification. Historical captures do not gain attribution from current settings.

## Concrete payload shapes

### `CodingAgentIdled`

```json
{
  "type": "CodingAgentIdled",
  "data": {
    "has_changes": true,
    "is_external_repo": false,
    "requires_restart": false,
    "cc_session_id": "abc123-…",
    "coding_agent": "claude-code",
    "worktree_path": "/Users/.../.lucidos/worktrees/thread-1a2b3c4d",
    "worktree_head_sha": "f6ae7364e…",
    "bg_bash_pending": false
  }
}
```

All fields except `coding_agent` are `#[serde(skip_serializing_if = ...)]`-gated, so they are missing from the wire at their zero value (`false` for bools, `None` for `Option`s). Read defensively: `payload.has_changes ?? false`, `payload.worktree_path ?? null`.

**A `condition` on this event can also name `thread_id`, which is NOT in the payload above.** The engine supplies it for every thread event, so `{ "thread_id": "<uuid>" }` scopes a wait or a trigger to one coding-agent session. It exists only at matching time. The event row stores the thread in the `events.thread_id` column, so a persisted payload does not hold it.

| Field | Type | When present |
|---|---|---|
| `has_changes` | `bool` | `true` iff the coding-agent branch has a non-empty net diff against its **diff base**, after filtering out runtime-only paths (`.lucidos/**` etc.; see `branch_changed_files` + `files_require_restart`). The diff base is the one the Diff button renders against (`default_diff_base`): `origin/<default>` when the local default branch has diverged, otherwise the local default. One function (`branch_changed_files`) gates the button and computes the diff, so the button never lights up on an empty diff. Carries forward from a prior idle when the turn made no new commits but the branch holds prior work. Moves the thread's change state between `none` and `unproposed` (the WaitingBanner Diff button), and leaves a `proposed` state alone. During a live turn, the worktree post-commit hook can move it earlier through `CodingAgentDiffChanged`. Only the aggregate `ChangeProposed` (non-empty `change_id`) makes it `proposed`. That follows only when the turn ended `Generated` and `idle_change_write` permits, which it does not while the thread holds a live event wait (ADR 0395). Stops, failures, aborts and mid-turn deaths never propose: they emit `ProposalWithheld` with reason `turn_incomplete` (see the "Changes" section of `thread-events.md`). |
| `is_external_repo` | `bool` | `true` for sessions on a repo imported via `RepositoryImported`, not the Lucidos engine repo. Authoritative for the `coding_agent_is_external_repo` column the WaitingBanner reads to swap Apply for Done/Archive. |
| `requires_restart` | `bool` | `true` iff at least one file in the same filtered list matches `files_require_restart` (Rust source, `Cargo.lock`, certain bundled assets). Informational only: `ChangeProposed.requires_restart` is authoritative for the `requires_restart` on a `proposed` change state. |
| `cc_session_id` | `Option<String>` | The CC CLI session id at idle. `None` for recovery-emitted idles with no live subprocess (the "no-branch" and "stuck-session" recovery paths). `CodingAgentSettingsChanged` also pins the id at `Init`, so a turn an engine restart interrupts before its first idle stays resumable. `lookup_latest_cc_session_id` reads the newest non-null id across both event types. |
| `coding_agent` | `CodingAgent` | `"claude-code"` or `"codex"` (kebab-case wire values). Defaults to `"claude-code"` on legacy DB rows. `#[serde(alias = "agent")]` decodes rows persisted under the older `agent` field name. |
| `reason` | `Option<String>` | **Usually absent.** Recovery stamps `"engine_restart_interrupt"` when a mid-turn-crashed session shows as "interrupted, click to continue" instead of auto-spawning. The frontend reads it to render the continue affordance. A *cache keep-alive* turn stamps `"cache_keep_alive"`: the agent is still waiting, so no event wait, trigger or parent callback treats that idle as the agent finishing, and the idle carries no `worktree_head_sha`. A *ceiling re-entry* turn stamps `"ceiling_reentry"` (ADR 0629): the agent only wrote its handoff, so no event wait or trigger reads that idle either. |
| `worktree_path` | `Option<String>` | Absolute path of the worktree the agent ran in, set by `run_session.rs` for normal turns. **`None`** on legacy rows, and when the worktree was `worktree remove --force`'d before the idle fired (the "stale session" cleanup path, the no-branch recovery path). |
| `worktree_head_sha` | `Option<String>` | `git rev-parse HEAD` in the worktree at idle time. The next spawn uses it to detect external user edits between turns. `None` on legacy rows, with no worktree, or when `git rev-parse` fails (e.g. a zero-commit branch). |
| `bg_bash_pending` | `bool` | **Recorded history only: it gates no proposal and drives no UI.** `true` iff the turn idled while a background task this thread started (a coding agent's `lucidos background-task run`) was still running and nobody had asked to stop it. A task the thread already stopped re-opens nothing (ADR 0369), so it does not count. It keeps nothing alive: the task's event wait re-opens the thread. A live event wait is what holds a proposal, not a background task: a clean idle on a thread holding one does not propose (ADR 0395). The wait's delivery or expiry re-opens the thread, and that turn's idle proposes. A wait canceled by Stop waiting or a stand-down re-opens nothing, so the engine then proposes the held work itself. **Startup recovery:** `propose_held_back_changes_on_startup` re-proposes at boot any idle coding-agent thread whose change state is `unproposed` with a committed diff. It emits `ChangeProposed` with `origin: Engine{ reason: StaleSession }`. It skips a thread that is not `unproposed`, is external-repo, holds a live event wait, or has lost its branch. A branch whose last turn did not finish, or that the proposal hold keeps (a missing or unapproved plan, or work that never ran `/harden`), gets `ProposalWithheld` instead. That emit happens only when the reason differs from the projected one, so a restart repeats nothing. It also skips one with no authored commits left, where only an engine back-merge of main remains. That three-dot diff would re-surface already-applied files as a phantom change. |

**Fires on:** every coding-agent turn that ended on a `Result` other than engine-shutdown abort. That covers natural completion, `Failed` (the coding agent errored, OOM, empty assistant text), and the user's `Cancel`, which is a turn boundary, not a terminator. Engine shutdown does NOT emit `CodingAgentIdled`: recovery resumes the session on next start.

**Cancel = Esc (resumable):** the `Cancel` button is a real *interrupt*, not a kill. `POST /api/v1/claude-code/stop` (default, `StopReason::UserStop`) routes through `interrupt_agent`, which forwards CC's native interrupt (pressing `Esc` in the CLI). CC winds down the turn and emits a `Result`. The engine emits `ResponseCanceled(UserStop)` **+** `CodingAgentIdled` carrying the `cc_session_id`. The branch is **kept** even with zero commits (`SessionEndAction::KeepCanceledBranch` in `finalize`), so the next message `--resume`s the *same* conversation on the *same* branch. Apply / Discard / Archive still hard-stop via `stop_agent`, each with its own terminator.

A bounded fallback escalates to the hard stop if CC ignores the interrupt for ~8s. Causes include a hung socket, or a control request ignored during a long tool. The watchdog skips while a tool is in flight. Even then the turn is stamped `Canceled(UserStop)`, so the branch is kept and the session stays best-effort resumable.

### Side questions are recorded, and hidden from every agent

A *side question* is asked from side-question mode. The question-mark toggle in the composer row, or ⌥↵, turns the mode on, and its round Ask button asks the box. It is answered beside any running turn, from the thread's own context, with no tools. It may carry images. It works in Claude Code, Codex and Lucidos Agent threads. Each takes one path, text and images alike:

- **Claude Code** asks a copy of the session: the session's own command resumed with `--no-session-persistence`, under a settings file whose PreToolUse hook refuses every tool. It keeps the session's tools and appended system prompt, so the prompt cache holds, and it writes nothing to the transcript. Its cost is recorded as `ContextCaptured` with purpose `side_question`. Claude Code's own `side_question` control request is not used, since it carries text only (ADR 0324).
- **Codex** asks an ephemeral app-server fork of its saved session with a synchronous catch-all PreToolUse denial. Shell, hosted search, image generation, browser, computer, connector, plugin and MCP tool sources are disabled. Before asking, `hooks/list` must verify the exact enabled guard as trusted or managed, without load errors. Approvals are declined and attempts interrupted; declined commands and patches permit retry, while completed or failed tools fail the ask. It never appends to the main session; usage records as `ContextCaptured` with purpose `side_question`, including failures after usage arrives. A CLI without verifiable protection shows update instructions (ADR 0684).
- **Lucidos Agent** makes one model call with a turn's system prompt, tools and history. A tool call is refused, never run. Its cost is recorded as `ContextCaptured` with purpose `side_question`.

The endpoints:

- **Ask:** `POST /api/v1/side-questions` with `{thread_id, side_question_id, question, image_hashes?}` returns `{answer}`. The client names `side_question_id` (a UUID), and `image_hashes` names blobs already uploaded to this workspace. A 400 refuses a draft, a coding-agent thread with no session yet, an empty question, or an unknown image. A 409 refuses an id still running or answered, while a failed one may be asked again (a card's Retry). No refusal records anything. An ask waits at most 120 seconds.
- **Dismiss:** `POST /api/v1/side-questions/dismiss` with `{thread_id, side_question_id}` returns `{ok: true}`, or 404 when no such ask exists on the thread.
- **Recorded:** `SideQuestionAsked` (with any `image_hashes`), then `SideQuestionAnswered` or `SideQuestionFailed`, and `SideQuestionDismissed`. So a card survives reload and shows on every device. Startup fails every ask a restart left unanswered.
- **Hidden:** no agent ever reads them. `events(action="query")`, triggers, event waits and every context builder leave them out, and they move no thread state.

A typed `/btw` is ordinary text. The chat route sends it as a normal message.

### `UserQuestionAsked`

```json
{
  "type": "UserQuestionAsked",
  "data": {
    "tool_use_id": "toolu_…",
    "cc_session_id": "abc123-…",
    "question": "Which approach should I take?",
    "options": [
      { "id": "opt-0", "label": "Approach A", "description": "…", "preview": "![A](artifacts/approach-a.png)" },
      { "id": "opt-1", "label": "Approach B" }
    ],
    "worktree_path": "/Users/.../worktrees/thread-…",
    "multi_select": false
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `tool_use_id` | `String` | The agent's identifier for this question: the unique key in DB and in `UserQuestionAnswered.tool_use_id`. Per-question sub-ids take the form `{outer}#q{i}`, so a multi-question batch from one tool call gets one DB row per card. |
| `cc_session_id` | `String` | The CC session id at intercept, which `--resume` pins to. **Empty string** for questions from the chat agent's `ask_user_question` tool (chat is in-process, with no subprocess to resume). |
| `question` | `String` | The prompt text shown to the user. **For permission requests, this is the human-readable rendering of the tool-use the coding agent asked permission for** (see "Question vs. permission" below). Filled strictly from the tool input's `question` field, never from the optional `header` chip-label. Tool-call schemas are advisory, so the model can omit a `required` field. The engine therefore rejects a batch with any entry missing a non-empty `question`, before any `UserQuestionAsked` is emitted, and the model re-asks. So this field is never empty or `(no question text)`. |
| `options` | `Vec<QuestionOption>` (default `[]`) | Each option is `{ id, label, description?, preview?, widget?, quiet?, quiet_reply? }`. `quiet: true` marks a *quiet option*: the Lucidos Agent and Codex tools take a `quiet` field, and Claude Code, whose native tool takes no extra field, starts the label with `[quiet]`, which the engine strips. `quiet_reply` is the option's *quiet reply*: the Lucidos Agent and Codex take a `quiet_reply` field, and Claude Code writes `[quiet: <reply>]`, which ends at the first `]`. A reply on an option that is not quiet refuses the card. `preview` is markdown the card shows under the option: a picture or a short text sample. It comes from the option's `preview` field in the tool input, which Claude Code's native tool has. `widget` is `{ app_id, params?, label? }`: the engine lifts one *widget embed* out of the description or preview into it, and the card draws that widget under the option's label (ADR 0415). A broken or doubled embed, a missing or non-widget id, another thread's widget that is not reusable, or params the manifest does not take refuse the whole batch before any card is written. So does an embed in the chat card's `message` whose label names one of the options: that sample belongs in its option. Empty for free-text-only prompts. **There is no text-entry option kind**: picking an option resolves to its `label`. So an agent-authored "Other, I'll type it" row hands that literal phrase back as the answer. The card names both real escapes itself: typing in the prompt arrives as a `FreeText` answer, and Cancel arrives as `Canceled`. Every question tool description forbids authoring an "Other" option. |
| `worktree_path` | `Option<String>` | The CC worktree at intercept. `--resume` needs it to find the session JSONL, since CC keys session storage by CWD. `None` for chat-channel questions and requests with no worktree context. |
| `multi_select` | `bool` (default `false`) | `true` when the user may pick multiple options. |
| `owner_approval` | `Option<{ verb, target_thread_id? }>` | Legacy: set only on the retired single-use card (ADR 0387). Nothing sets it now. A card Lucidos words in place of the agent's carries `permission_request` instead (ADR 0688). |

The engine adds one thing to the tool input before it emits the event. Each
workspace picture in `question`, `description` or `preview` gets an *image size
hint* (`system-knowhow/glossary.md`), so `![A](artifacts/approach-a.png)`
arrives as `![A](artifacts/approach-a.png#1600x1200)`. A trigger `condition`
matching those fields sees the hint.

