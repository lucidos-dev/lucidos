---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos successor start --narrative <file>`

Continue this thread in a *successor session*: a fresh session in this same thread, with a clean context. Use it when the user says "handoff this session" or picks an `Implement in a fresh session` option. In a Lucidos thread, advice from anywhere to run `/clear` or to write a handoff file means this too. Wraps `POST /api/v1/threads/:id/successor` for `$LUCIDOS_THREAD_ID`.

1. **Write a *handoff narrative*** to a file outside the repo. It needs one markdown heading for each of: Outcomes, Decisions, Gotchas, State, Next step, Files. The engine names any it lacks and refuses the handoff.
2. **Run the command, then end your turn.** The successor session starts in this thread once the turn ends, and reads the narrative first.

```bash
lucidos successor start --narrative "$TMPDIR/handoff.md"

# At an automatic checkpoint, name it again, so the session skips that checkpoint:
lucidos successor start --narrative "$TMPDIR/handoff.md" --at phase-commit --progress 2/5

# Build on another rung (the plan card's pick), as `lucidos intelligence-ladder` prints it:
lucidos successor start --narrative "$TMPDIR/handoff.md" \
  --coding-agent-model <model id> --reasoning-effort <level>
```

**The successor session runs on this thread's implementer unless the command names another.** The three flags are spelled as on `spawn-thread`:

| Flag | Omitted |
|---|---|
| `--coding-agent <claude-code\|codex>` | this thread's backend |
| `--coding-agent-model <id>` | this thread's model on the same backend, else the backend's default implementer. Naming this thread's model without a `[1m]` suffix keeps its exact id, `[1m]` included, so the session reads the same prompt cache |
| `--reasoning-effort <low\|medium\|high\|xhigh\|max>` | this thread's effort on the same backend while the new model accepts it, else the backend's default implementer |

Pass model ids, not picker labels. The engine refuses a model or effort the backend does not offer, before anything is recorded, with reason `implementer-not-offered` and the offered list. A model or effort the agent names must also be a rung on the intelligence ladder. Otherwise the engine refuses with reason `off-intelligence-ladder`, and the command prints the ladder. Carrying this thread's own implementer over is never refused, even off the ladder, since the owner may have picked it by hand. The command prints the implementer the session runs on.

What the handoff does:

- **It records `SessionHandedOff`** and creates no thread, no branch and no change. The thread keeps its worktree, branch, plan marker and change, uncommitted work included.
- **The current session ends with its turn**, and the successor session then starts in the same thread. Its first message is the narrative, and its `SessionStarted` records continuity `handoff`. Another message arriving first starts it too, with the narrative ahead of that message.
- **Past the *context ceiling*, a natural turn end waits for input.** If ceiling re-entry interrupted running work, the agent’s narrative handoff starts after its turn instead. An engine brief keeps waiting. The input is a message, a finished task, a wake or a child's report. The input follows the narrative in the first message. Input that arrives meanwhile waits for the new session.
- **The narrative ends on a "Still running" list**: the background tasks, event waits and working children the session leaves behind. Their results reach the new session.
- **The backend may change.** A session on Claude Code may hand off to Codex, and back. The thread's backend follows its newest session.
- **A parent sees nothing change.** It keeps waiting on this thread, and hears from it once, at the real end.
- **No authority is needed**, and no permission card is ever asked for. A thread can hand off only itself.
- **Refused** for a thread that is not a coding-agent thread, or an archived thread. An *integrated sub-thread* hands off like any other: its successor session keeps the branch its parent merges. A thread with a handoff already pending is refused with reason `handoff-pending`. A handoff past the context ceiling is refused with reason `ceiling-lifted` when a person raised the ceiling first. Hand off again if you still mean to. A second request racing a handoff waits for the first, then is refused.
  - Also refused while the thread applies a change or has one in the *landing lane*.
  - A lane thread whose batch landed or failed is refused with reason `lane-ended`, since no session runs on it again.

The route's body takes `coding_agent`, `coding_agent_model` and `reasoning_effort` beside the narrative, plus `checkpoint` and `progress`. Its reply carries `{implementer, starts}`. For a ceiling handoff, `starts` is `after-this-turn` for an agent narrative whose re-entry interrupted running work, and `on-next-input` otherwise. A request typed at the Successor card starts `now`. Otherwise it is `after-this-turn` when the thread's own agent called, and `now` for any other caller.

`GET /api/v1/threads/:id/context` returns `{input_tokens, can_hand_off, offer, applied, implementer, card, ceiling_handoff}`. `input_tokens` is the context size, and `can_hand_off` says whether the thread may hand off at all. `offer` is the size decision the plan card reads.

`card` is the *Successor card*'s verdict, `{warm, cold, cold_at, fresh_ratio}`. `warm` and `cold` say whether the handoff estimate says a handoff pays while the prompt cache is warm and once it is cold. `cold_at` is when the cache goes cold. `fresh_ratio` is how many times a fresh session's context the thread holds. It is absent when the engine cannot make a reading.

`applied` says whether the thread has an applied change. `implementer` is `{coding_agent, model, reasoning_effort}` for a coding-agent thread, a `null` model or effort meaning the backend's default, and `null` for any other thread. A thread whose handoff is pending, or that handed off to a legacy successor thread, reads `can_hand_off` and `offer` false, and shows no card.

The *Successor card* uses two other routes, both for a user's device and refused for an agent. `POST /api/v1/threads/:id/successor-offer/continue` takes `{message?, coding_agent?, coding_agent_model?, reasoning_effort?}`. The engine picks the path by the prompt cache's warmth and answers `{path}`. On `narrative` (a warm cache) it records `SuccessorRequested` and asks the agent for its *handoff narrative*. The agent's `lucidos successor start` then picks up that request and implementer. On `brief` (a cold cache) it starts the successor session at once from a *handoff brief*, and also answers `{implementer, starts}`.

A thread that is not idle is refused.

`POST /api/v1/threads/:id/successor-offer/decline` records *Stay here* as `SuccessorOfferDeclined`.

`ceiling_handoff` is present while a handoff at the *context ceiling* waits for the next input: `{opening, crossing, raises_to}`. `crossing` holds the size and the ceiling it crossed. `raises_to` is the ceiling *Return to the old session* sets, in tokens: the session's size plus 100k. It is `null` when the window cap keeps every ceiling under that size.

`POST /api/v1/threads/:id/successor/withdraw` is *Return to the old session*, for a person only. It records a raising `ContextCeilingSet`, a `SuccessorOfferDeclined` at the session's size, then `SessionHandoffWithdrawn`, and answers `{ceiling}`. The next input resumes the old session. An agent gets a 403 with reason `withdraw-needs-a-person`. A thread with no waiting ceiling handoff gets a 409 `no-ceiling-handoff`, and one the window cap holds gets a 409 `ceiling-cannot-rise`. Once a run holds the thread, the successor session is already starting, and the withdraw gets a 409 `handoff-starting`.

`POST /api/v1/threads/:id/handoff-checkpoints` takes `{name, progress?, offered?}` and returns the same reading plus `handoff`: `{name, progress, pays, estimate}`. `pays` is `null` when the engine could not make a reading, such as a model with no price row. It is `false` while the thread holds a pending, applying or landing change. `estimate` holds `warmth`, `units_left`, `calls_per_unit`, `calls_left`, `fresh_tokens`, `saving_usd` and `cost_usd`. A malformed name or progress is a 400, and a thread records only its own runs.

### `lucidos frontend-preview start [--thread-id <uuid>]` / `lucidos frontend-preview stop` / `lucidos frontend-preview status`

Start, stop, or inspect the **frontend preview**: a Vite dev server the engine supervises inside a coding-agent worktree, on its own port. So a TypeScript or CSS change is visible in the real app **before Apply**. Wraps `POST /api/v1/frontend-preview/start` / `/stop` and `GET /api/v1/frontend-preview`. Development only, and refused on a packaged install.

```bash
# Inside a coding-agent worktree: preview THIS thread's branch. --thread-id
# defaults to $LUCIDOS_THREAD_ID, which every coding-agent subprocess carries.
lucidos frontend-preview start
# → Frontend preview running for thread <uuid> at https://localhost:6173/

# Point it at a different thread's worktree (there is one slot, so this replaces
# whatever was running).
lucidos frontend-preview start --thread-id 2951200f-0652-4ee2-baa3-433d608983d8

lucidos frontend-preview status
lucidos frontend-preview stop
```

**Why the CLI rather than starting `vite` yourself:** when a coding-agent turn ends, the engine kills every process the session started, and a dev server the agent started dies with it. The engine owns the process so the preview outlives the turn.

**One slot per workspace.** `start` on another thread moves the preview rather than adding a second one.

`start` refuses, by name, in four cases:

- the thread has no worktree;
- the worktree is not a Lucidos-source one (an app or external-repo thread has no frontend to preview);
- its dependencies were never provisioned;
- no workspace gateway launched the engine. It answers only once Vite is actually serving, so the printed URL is live when you paste it into a reply.

**The printed URL uses the host the CLI reached the engine on**, which from inside the worktree is `localhost`. That is right for the host machine and wrong for a phone. The in-app control (the coding-agent control menu's *Frontend preview* section) builds the URL from the page's own location, so it resolves on a tailnet. Point the user at that control rather than pasting a `localhost` URL. It also carries the device id, which the CLI cannot know. Without it the preview renders with none of that device's scoped preferences.

**Other devices reach the preview only when the gateway has a user certificate** (`LUCIDOS_TLS_CERT` / `LUCIDOS_TLS_KEY`). Without one, Vite serves `https://localhost` on the gateway machine alone, because the local certificate names nothing else. The status then carries `loopback_only: true`, and the in-app control says so instead of offering a dead link.

The preview registers **no service worker** and cannot do push: a dev server emits unhashed module URLs a worker would cache past a hot update.

**The preview needs a paired device.** It reaches the engine through the workspace gateway, and serves its own files only to a browser holding the gateway's device cookie. So it opens in a browser that already has Lucidos open, and answers 401 to anything else, a `curl` included. See ADR 0055 and ADR 0267.

### `lucidos knowhow list`

List the merged user + system-knowhow catalog. Wraps `GET /api/v1/knowhow` and echoes the engine's payload verbatim: `{ "knowhow": [{ "id", "name", "description" }] }`. Engine-shipped reference docs carry the `system-knowhow/` id prefix; user-curated knowhow uses its path under `data/knowhow/` without `.md`. Read `.knowhow[].id` to find the id to pass to `read`.

The catalog holds knowhow *docs*. A doc's own reference files sit below the listed depth, so `list` does not show them and `read` still takes their full id. See `system-knowhow/building-knowhow.md` § "Where the file goes".

```bash
$ lucidos knowhow list
{"knowhow":[{"id":"audit-checklist","name":"Audit checklist","description":"..."},{"id":"system-knowhow/building-an-app","name":"Building an App","description":"Use when the user wants to build..."},...]}
```

### `lucidos knowhow read <id>`

Read one knowhow doc's full content by id. Wraps `GET /api/v1/knowhow/read?id=<id>` and prints the same `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]` block the chat agent's `load_knowhow` tool returns. Exit non-zero (with the engine's not-found sentinel on stderr) when the id resolves to nothing. A successful read records one `KnowhowRead` per doc per day, so the *workspace prompt footprint* sees the doc as read.

A doc over the 24 KB knowhow cap still prints whole, after a first line naming its size. A shell may cut such a doc before the end, so the note says to split it (`system-knowhow/building-knowhow` § Size). A shipped doc split into parts lists their ids under `## Parts`. Read the part you need.

```bash
$ lucidos knowhow read system-knowhow/building-an-app
[SYSTEM-KNOWHOW: Building an App]
# Building an App
…
[END SYSTEM-KNOWHOW]
```

**Why this exists.** The chat Lucidos Agent loads `system-knowhow/*.md` with its in-process `load_knowhow` tool, using the same id. An *app coding-agent thread* runs in a sparse-checkout *worktree* narrowed to one `data/apps/<id>/` folder, with neither the files nor that tool. This subcommand is how such a session (Claude Code or Codex) pulls the same guidance on demand:

- `system-knowhow/building-an-app`: when an app is the right answer, scaffolding defaults, common mistakes.
- `system-knowhow/js-sdk`: the `lucidos.*` SDK surface.
- `system-knowhow/best-practices`: file layout, where app data lives.

Load the relevant knowhow before writing app code, rather than guessing at the SDK surface or data paths. Any bash or Python subprocess without in-process tools uses this CLI too.

