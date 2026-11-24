---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos changes apply <change-id>`

Apply a pending *change* (a coding-agent-proposed branch waiting on the Apply button). Wraps `POST /api/v1/changes/<id>/apply` and echoes the engine's typed apply-result JSON to stdout. Get the id from `lucidos changes list` (`.pending[].id`).

```bash
$ lucidos changes apply fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec
{"status":"applied","change_id":"fbcc4a3a-...","thread_id":"1c1c34ef-...","message":"Change applied.","restart_required":false,"applied_commit":"9b1a...","previous_commit":"2a3b...","commits_applied":3,"files_changed":5}
```

The result also reports `version_build`: `started` (started or joined), `deferred` (waiting for active coding threads), or `none`. With `deferred`, `deferred_coding_threads` gives the count, and `message` says the new version waits for those threads to finish. The change is still applied. **Build now** overrides the hold; otherwise it starts after the threads finish. Older engines can omit these fields.

> **In-thread agent:** the chat Lucidos Agent has the `changes(action="apply")` LLM tool. It runs the same apply pipeline **in-process**. It stamps the apply as the agent, linked to the applying thread, so the route popover never mislabels it as "You". Use `changes(action="apply")` from a chat or trigger thread. Use this CLI from a `script:`-typed trigger or a bash or Python subprocess, which cannot call the in-process tool.

> **Both refusals below reach the LLM tool too.** `changes(action="apply")` asks the same gate this CLI's route asks (ADR 0233). A thread that has not settled, or a change with no files left, is refused there as well. The tool error names the way forward. It offers `changes(action="apply_when_settled")` only for a *settling* thread, because `changes(action="apply_when_settled")` refuses one parked on a question.

> **Applying work that has not finished:** the agent also has two *standing apply* actions. `changes(action="apply_when_settled")` takes one thread's change, applied the moment that thread finishes. `changes(action="apply_as_they_settle")` takes everything pending that has settled, plus every *settling* thread as each one lands. Both arm the same instruction the Apply control arms from the UI. Both wait through an event wait, and drop with a report if a thread stops on a question or fails. LLM-only, with no CLI form.

> **Only a proposal lands.** A standing apply also drops when the settle withholds the work (`ProposalWithheld`), and the report names the reason.

> **Nothing to wait for, nothing armed.** `changes(action="apply_when_settled")` refuses a thread that has already settled with nothing pending, parked on a question, or failed. So never re-arm a thread whose change just landed.

> **Neither reaches a repo Lucidos does not apply into.** An *external-repo coding-agent thread* proposes no *change* at all, so `changes(action="apply_when_settled")` refuses one and the `changes(action="apply_as_they_settle")` sweep passes it over. That work is reviewed and pushed from the repo itself.

> **Taking one back:** `changes(action="cancel_standing_apply")` is the off for both. With a `thread_id` it cancels that thread's instruction. Without one it cancels every standing apply in the workspace. The Changes panel's Not finished toggle cancels only the changes it lists. It stops future applies only: a change already merging or hardening keeps going, and nothing already applied is reverted. Cancelling a running Apply All batch is a different action.

> **Setting a change aside:** `set_aside` keeps a pending change for later, out of Review and Apply All, and lets its thread be archived. On a live thread it also puts the thread on *hold*, so the change lists under On hold and Resume returns it. `bring_back` returns it to pending. Neither loses the branch. `set_aside` is refused where Discard is, and `apply` refuses a set-aside change until it is brought back. LLM-only, with no CLI form.

> **Bringing back unfinished work:** an incomplete set-aside row, such as one the archive net filed, never returns to pending. `bring_back` withdraws it and decides again, and proposes under a fresh id only if the last turn finished.

> **All six ask the same authority question the CLI does.** Applying a change from your own subtree needs nothing. Anything wider is the *workspace owner*'s, and the tool returns the refusal above rather than applying. `changes(action="apply_as_they_settle")` is always wider, since the sweep reaches every thread in the workspace, and so is a `changes(action="cancel_standing_apply")` naming no thread.

The response carries:

| Field | Meaning |
|---|---|
| `status` | `applied`, `noop`, `hardening`, `conflict` or `queued` (see `docs/apply-change-api.md` for the full table). `queued` means the *landing lane* holds a Lucidos-source change, and `ChangeApplied` follows when it lands. |
| `applied_commit` | 40-char SHA on `main` AFTER the merge (present on `applied` and idempotent `noop`) |
| `previous_commit` | 40-char SHA on `main` BEFORE the merge |
| `commits_applied` | Number of commits added to `main` (0 for `noop`) |
| `restart_required` | `true` when the change needs a new engine version. Apply never restarts Lucidos: it builds the new version in the background, and the user taps "Switch to new version" to restart onto it. Never tell them an apply restarts Lucidos. |
| `conflict_thread_id` / `review_thread_id` | Thread to focus when `status` is `conflict` / `hardening` |

The CLI prints the JSON verbatim on stdout. It exits non-zero on a transport error or a 4xx, with the engine's error body on stderr, like `lucidos proxy --fail`.

Two 409s are refusals rather than errors, and both name the resolution:

- The change's thread is still working: wait for it to idle.
- The change has **no file changes left** (`file_count` is 0). Its branch's commits cancelled out, so there is nothing to merge: discard it with the Discard button. A script applying changes treats a zero-`file_count` entry in `lucidos changes list` as "nothing to apply", not a change to retry.

A 403 is the third refusal, about authority rather than state. Applying a change acts on the thread that proposed it, so a change from your own subtree needs nothing. A change from anywhere else is the *workspace owner*'s to apply. You may apply it only with their standing instruction: a turn they opened, or a trigger firing they authorized. Discard answers the same way. The body names both shapes, so read it rather than retrying.

#### Why the CLI and not hand-rolled urllib / curl

A hand-rolled request gets two things wrong that the script cannot see. And the
rule at the top of this file forbids raw HTTP to the engine at all.

**It loses your identity.** The CLI forwards the subprocess-origin header
(`x-lucidos-agent-origin-token`). The engine reads it to stamp the
`ChangeApplied` event as `Api { mode: Agent, source_thread_id }`. The token is
*thread-bound*: the engine mints one per spawn and reads the spawning thread off
the token, so the popover links back to the thread that acted. Without it the
engine sees an unattributed API client.

Your agent action is recorded as an
anonymous one, and on the chat path it is refused outright. A `run_python` block
calling `urllib.request.urlopen(".../api/v1/changes/<id>/apply")` hits this,
because urllib does not read the env var.

An origin token lasts for one engine boot. The shared mutation identity gate
returns 401 with `reason: "origin_token_stale"` when an origin-token header
is present but no identity resolves. The message names a possible engine
restart and asks you to run the work again. Unknown tokens get the same
refusal; the engine cannot prove that a token came from an earlier boot.

The CLI prints the server body unchanged. There is no token refresh path.

With no credential presented, the existing 401 message still lists the four
credentials.

**It can reach the wrong engine.** The CLI resolves this workspace's engine and
asserts its workspace, so a wrong port comes back as a 409 naming the right
one. A hand-built `https://localhost:<port>/...` asserts nothing, and whichever
engine holds that port serves it in full.

```python
# WRONG: unattributed, and aimed at a port you guessed
import ssl, urllib.request as r
ctx = ssl._create_unverified_context()  # self-signed cert
r.urlopen(r.Request(f"https://localhost:{port}/api/v1/changes/{cid}/apply", method="POST"), context=ctx)

# RIGHT: the CLI forwards the headers and resolves the engine, so the UI says
# "Lucidos Agent" with the source thread linked
import subprocess
subprocess.run(["lucidos", "changes", "apply", cid], check=True)
```

The same rule applies to bash:

```bash
# WRONG: bare curl from inside a run_bash tool
curl -k -X POST "https://localhost:$LUCIDOS_API_PORT/api/v1/changes/$CID/apply"

# RIGHT: the CLI handles the headers and the target
lucidos changes apply "$CID"
```

**If there is no CLI subcommand for what you want, that is the answer.** Do not
substitute raw HTTP (see the top of this file). Only test harnesses and
external tools that cannot shell out speak to `/api/v1` directly, and neither is
an agent working around a refusal.

If you need the underlying URL for an operation the CLI covers, use
`$LUCIDOS_API_BASE_URL`, never one built from `$LUCIDOS_API_PORT`. See
§ "Workspace resolution" for why. See `docs/apply-change-api.md` for the apply
response shape and the full workflow.

### `lucidos landing`

Read the *landing setting* for the current workspace.

### `lucidos landing setting`

Prints `here` if the workspace is a *lander*, or `another-host` if it hands
queued Lucidos-source changes to the lander through `origin`. To change
the setting, use **Settings → Coding Agents**.

### `lucidos main-check status [--json]`

Where the *main check* stands: the GitHub run on a stripped copy of `main` after changes landed. Wraps `GET /api/v1/main-check`. One fact per line:

```bash
$ lucidos main-check status
Main is red: Failed: engine, browser chromium.
Range: 4f1c2a9e0b7d..9b1a77c3e210
Run: https://github.com/lucidos-dev/lucidos/actions/runs/12345
Fix thread: [Fix main](thread:myws/6fa459ea-ee8a-3ca4-894e-db77e160355e)
```

The state is one of:

| State | Meaning |
|---|---|
| `running` | A check runs now. The line names the sha it tests and the range it covers, then its run. |
| `passed` | The last check passed, and nothing landed since. |
| `red` | A failed check remains open. Newer checks continue; one main repair owns the findings. |
| `idle` | No check runs and main is not red. The next landing starts one. |
| `unavailable` | No check starts here: no release library, no `lucidos` remote, or a *main-check setting* other than `here`. |

The status also prints the *main-check setting*, the main repair state, what it waits on, and current failed and flaky tests. A waiting repair has no thread yet. Fix now in the Lucidos menu starts it at once; there is no agent CLI form.

`--json` prints the whole snapshot, including `repair`, `failures` and `flaky`. Exit non-zero on a transport or HTTP error.

**Read it before fixing a failure that also fails on `main`.** When main is red with the same failure, the fix thread owns it. Do not fix it on your branch: say so, and carry on with your own change. There is no CLI form for closing a red. Only the maintainer closes one, from the Lucidos menu.

### `lucidos main-check setting`

Prints the *main-check setting*, one word: `here`, `another-host` or `off`. Unset, the engine prints its default for this checkout. It prints nothing before the engine restored the check. `/harden` reads it to choose the round: under `here` and `another-host` it leaves the slow suites to the main check. Only the user changes it, in Settings → Coding Agents.

### `lucidos hardened mark [--tier fast|full]` / `query` / `sha` / `tier` / `expected-tier` / `integrated-heads`

Record or read the hardening marker (see *Hardening* in the glossary): the HEAD SHA the last `/harden` run covered on a *Lucidos-source* coding-agent branch, and the tier it ran. Every subcommand resolves repo_root and branch from `$PWD`'s git worktree. They wrap `POST /api/v1/internal/mark-hardened`, `GET /api/v1/internal/hardened-state` and `GET /api/v1/internal/integrated-heads`. `mark` must run from a Lucidos-spawned process (a coding-agent session or an engine-spawned script) or from a shell on the machine holding the machine-local token. The engine refuses anyone else with 403, including a request that came through the gateway.

```bash
lucidos hardened mark              # /harden Phase 5: record HEAD as hardened, full tier
lucidos hardened mark --tier fast  # an integrated sub-thread's /harden Phase 5
lucidos hardened query             # FRESH (HEAD matches), STALE (commits since), MISSING, or NOT_NEEDED
lucidos hardened sha               # the recorded SHA, fresh or stale; exit 1 with no marker
lucidos hardened tier              # the recorded tier, fast or full; exit 1 with no marker
lucidos hardened expected-tier     # the tier /harden runs here: fast or full
lucidos hardened integrated-heads  # your integrated children's hardened heads, one per line
```

**A fast marker counts only for an integrated sub-thread.** The engine refuses `--tier fast` on any other branch with a 409, since it would not count there. A full marker counts everywhere. `query` answers for the tier the branch needs, so a fast marker on a branch that needs the full run reads `MISSING`. A branch that lands implementation plans only reads `NOT_NEEDED` in place of `STALE` or `MISSING`: Apply never hardens it, so `/harden` stops there.

Run from a coding-agent session, `mark` also stops every background task the thread still has running, since this hardening supersedes it. Their completions will not re-open the thread (ADR 0369). It then prints what it stopped, and every wait still live on the thread:

```text
Hardening recorded: my-branch 1a2b3c4d5e6f
Stopped background task 5f0c2e1a-… (make lint): this hardening supersedes it, and its completion will not re-open this thread.
Still waiting on: the release build to finish. Ending your turn leaves this thread waiting, and Apply stays withheld until that resolves. …
```

A wait still live means the session is not finished: say so, or stand it down with `lucidos event-waits cancel`. From your own shell, `mark` stops nothing.

`query` prints `FRESH`, `STALE`, `MISSING` or `NOT_NEEDED`, which `/harden` Phase 0 and the `pre-push.sh` hook compare literally. `sha` feeds `/harden`'s merge-only check: is every commit since that SHA a merge of main? Apply consumes the marker, so a freshly applied branch reads `MISSING`. `/harden` drives these, so you rarely call them by hand.

### `lucidos planned start [--kind design|implementation]`

`lucidos planned start --kind design|implementation` records `PlanStarted` on the calling thread before writing the plan. The default kind is `implementation`. It grants no edit permission and requires a Lucidos-spawned thread. Repeating the same active start is idempotent, including concurrent requests; a kind switch or an intervening approval or landing begins a new stage.

