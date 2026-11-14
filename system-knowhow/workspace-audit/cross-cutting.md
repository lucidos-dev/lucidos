---
name: Workspace audit: cross-cutting
description: The cross-cutting audit section: credentials in workspace code, broken references, duplicates, removed CLI flags, removed thread-summary fields, retired LLM tools and the retired work-item names.
sources: ["system-knowhow/js-sdk.md", "system-knowhow/lucidos-cli.md", "system-knowhow/intent-registry.md", "system-knowhow/triggers.md", "crates/lucidos-cli/src/*", "crates/lucidos-engine/src/llm/tools/*", "crates/lucidos-engine/src/engine/tools/*", "crates/lucidos-engine/src/api/threads*", "crates/lucidos-engine/src/core/store/threads/*"]
---

# Workspace audit section: cross-cutting

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan removed-flags "spawn-thread[^|]*--(parent|cc-model)|run_coding_agent\([^)]*repo[[:space:]]*=|threads[\\\"',[:space:]]+(list|count)[^|]*--has-diff" $all
scan removed-fields "coding_agent_(applying|has_diff|proposed|incomplete|requires_restart)" $all
scan cred-env "CRED_[A-Z0-9_]+" --exclude-dir=auth $all
scan auth-header "Authorization|X-API-Key" $inc apps
scan retired-tools "execute_intent" $all
scan retired-work-items "lucidos\.workItems|/api/v1/work-items" $all
```

## Checks

- **Direct-port device authority.** Check rig recipes against `system-knowhow/lucidos-cli.md` § `lucidos workspaces device-session`.
  Flag registration alone, permission reuse across mint parameters, or a session reused after re-keying as **broken**.

- **Event query boundaries treated as inclusive.** CLI `events query` and `events count` use exclusive `since` and `until` bounds.
  Audit inventory windows against that contract before asserting event parity.

- **Thread verdicts from history.** Read current `liveness` before calling a thread stalled, failed or done.
  Use the thread-scoped events header or a thread list/search row; see `system-knowhow/glossary.md` § Thread liveness.

- **Confusing acceptance with message recording.** Ordinary HTTP sends acknowledge durable acceptance, not execution.
  Check retries against `system-knowhow/thread-queue.md`; preserve the original wire intent after an unknown outcome.
  Flag recipes clearing a newer compose draft after replay as **broken**.

- **Fit-only credentials mistaken for owner authority.** Check renderer recipes against `docs/widget-fit-renderer.md`.
  Flag credentials in shell state, frame messages, URLs, saved evidence or general request headers as **broken**.
  Only the selected main-frame fit-report request may carry the credential; it grants no Device or owner authority.

- **Saved-draft snapshots mistaken for authority.** Review draft recipes against `system-knowhow/lucidos-cli.md` § `lucidos threads drafts`.
  A snapshot's revision and epoch identify saved content; neither grants permission to submit it.
  Flag direct compose clearing or arbitrary Human-mode POSTs offered as submission shortcuts as **broken**.
  Submission goes through `lucidos threads submit-draft` (§ `lucidos threads submit-draft`), with a standing instruction or a permission.
  Flag a recipe that submits a draft on reach alone, even over its own child, as **broken**.

- **Agent text standing in for the owner's answer.** Check recipes against `system-knowhow/lucidos-cli.md` § `lucidos permissions list` and § `lucidos threads pause-automation`.
  Only the owner's pick on a permission request or a batch grants, and only the owner unpauses.
  Flag a recipe that tells an agent to grant itself a permission, unpause a thread, or send a draft without the owner's authority as **broken**.

- **Assuming every announced artifact becomes memory.** Check recipes against `docs/system-events.md` § Artifact provenance and provider holds.
  Unattended trigger writes stay readable but do not create workspace memory leaves.
  Flag a recipe relying on automatic recall of those files as **drift**; have it read the file explicitly.

- **Pairing links in recipes.** Generate fresh device links with `lucidos pair`; never copy credentials into query URLs or workspace files.

- **Cleanup authority.** Workspace review is read-only. Refuse recipes that use local tokens for manual deletion or infer ownership from workspace names.

- **Restore authority inferred from a caller-provided workspace.** Review snapshot recipes against `system-knowhow/backups/workspace-snapshots.md` and `system-knowhow/lucidos-cli.md`.
  Flag self-restore instructions or retries after unknown delivery as **broken**.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

- **A credential written into workspace code.** One rule over four surfaces, so it lives here, not in the `apps`, `knowhow` and `scripts` sections. Where the workspace calls an external API the engine holds a credential for, use its builtin proxy or configure a credential-backed entry in `data/config/apis.json`.
  Builtin sign-ins stay inside the engine; see `system-knowhow/js-sdk.md` § `lucidos.proxy`. Flag:

  - An inline auth header. `curl -H "Authorization: Bearer $CRED_<NAME>"`, a pasted literal token, an `Authorization` / `X-API-Key` / `Bearer` header built in JS, or `requests.get(url, headers={"Authorization": ...})` and its equivalents.
  - Prose telling a future session to "set `$CRED_X`", or to read the credential out of the environment.

  Walk `data/apps/**`, `data/scripts/**`, every `scripts/` under a trigger, and the fenced `bash` / `sh` / `python` / `js` / `ts` blocks in `data/knowhow/**/*.md`.

  The fix differs only by caller: `lucidos.proxy(name).fetch(...)` in an app, `lucidos proxy <name>` in a script, `proxy_request` for the LLM. Severity: **drift**, since the call still works. Give a pasted literal token its own line in the report: the file is git-tracked, so the credential needs rotating, not just rerouting. Owns the rule: `system-knowhow/js-sdk.md` § `lucidos.proxy` and `system-knowhow/lucidos-cli.md` § `lucidos proxy`.

- Broken references: missing `knowhow:` ID, missing script path, manifest pointing at a deleted asset.
- Model request recipes use `lucidos.modelRequest` and `lucidos.proxy` without
  reading credentials. See `system-knowhow/js-sdk.md` for the selection contract.
- Duplicated content: same knowhow text in two files, same script copied between apps.
- Patterns the source-of-truth files explicitly mark deprecated. Grep for the old form and point at the doc that flags it.
- **Removed CLI flags and tool args still passed by workspace code.** These were
  removed after a deprecation window, and a recipe passing one now fails with a
  rename error. Grep each form across `data/knowhow/**/*.md` fenced code blocks,
  `data/scripts/**`, each trigger's `scripts/`, and `data/apps/**`, and recommend
  the replacement. Severity: **broken** (the call errors on next use). Currently
  removed:
  - Removed `lucidos spawn-thread --parent` → `--relation child` (a same-workspace
    parent-with-callback spawn). Do NOT flag `threads list --parent <uuid>` or
    `threads count --parent <uuid>`, which are current, unrelated filters.
  - Removed `lucidos spawn-thread --cc-model` → `--coding-agent-model` (same value).
  - `repo` passed to the `run_coding_agent` tool → `folder` (which also accepts a
    registered repo name). Do NOT flag the current `lucidos spawn-thread --repo`
    flag, which is a different, live argument.
  - Removed `lucidos threads list --has-diff` and `threads count --has-diff`, and the
    `threads` tool's `has_diff` arg → `--change-state` / `change_state`. The
    tool and the HTTP route refuse a stale `has_diff` by name.
    `--has-diff false` becomes `--change-state none`. `--has-diff` covered both
    `unproposed` and `proposed`, so ask which one the code means. The scan
    finds only the CLI form: read knowhow `threads` calls for `has_diff` by
    hand. Do NOT flag `has_diff` on `CodingAgentDiffChanged`, which is current.
- **Removed thread-summary fields still read by workspace code.** A thread
  summary comes from `lucidos.threads.list`, `lucidos threads list` or the
  `threads(action="list")` tool. A removed field reads as `undefined` in JS, so the code
  quietly takes its falsy branch. Grep the same four surfaces as the flags above. Severity: **stale**, or **broken** for a Python `row["field"]`,
  which raises on the missing key. Currently removed:
  - `coding_agent_applying`. There is no replacement: it never tracked a live
    merge reliably. Delete the read and whatever branches on it.
  - `coding_agent_has_diff`, `coding_agent_proposed`, `coding_agent_incomplete`
    and `coding_agent_requires_restart` → the one `coding_agent_change_state`
    object. Read its `kind`: `has_diff` is `kind != "none"`, and `proposed` is
    `kind == "proposed"`. `requires_restart` now lives on the `proposed`
    object. Unfinished work is `unproposed` with `reason: "turn_incomplete"`.
    Do NOT flag SQL that reads `thread_summaries.coding_agent_requires_restart`:
    that column still exists.
  - `waiting_children_count` → `awaited_children_count` (ADR 0672). The new
    count is wider: it also counts running children, and children that wait on
    a child of their own. Code that meant only idle children holding an event
    wait lists the children and reads each one's `live_event_wait_count`.
- **Retired LLM tools still named by workspace code.** The `retired-tools`
  scan finds every mention across `data/apps/**`, `data/triggers/**`,
  `data/knowhow/**` and `data/scripts/**`. Fix every hit, so none survives the
  run. Currently retired:
  - The retired `execute_intent` tool. An intent now runs in the thread that asks for it: load
    it with `load_knowhow(<intent id>)` and do its steps in the same turn.
    Owns the rule: `system-knowhow/intent-registry.md`.
    - **broken**: an instruction to call the retired tool, such as "use `execute_intent` to
      run the digest". The call now fails with a hint.
    - **stale**: a passing mention of the retired tool that tells nobody to call it,
      such as "runs through `execute_intent`" (the retired tool).
- **The retired work-item names.** The `retired-work-items` scan finds the
  retired `lucidos.workItems` namespace and `/api/v1/work-items` routes (ADR
  0649). Both are gone, with no alias, so a hit is **broken**. Recommend
  `lucidos.threads.users` and `/api/v1/threads/users`, per
  `system-knowhow/js-sdk.md` § "lucidos.threads.users". A hit in the Factory
  app means the team copied it before the rename: copy its three files again
  (`system-knowhow/team-workspace.md` § 5).

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding, and the root's § Inline or a coding agent picks its
route.

### Rewriting a retired tool call

**Route: inline.** Each row rewrites prose or one call, and changes no control
flow.

| Old | New |
|---|---|
| An instruction to call the retired `execute_intent(intent_id: "<id>")` | `load_knowhow(id: "<id>")`, then do its steps in this turn |
| An instruction to run the work in a separate thread through the retired `execute_intent` | `run_thread`, only where the work truly needs its own thread |
| A passing mention of the retired `execute_intent` | the same sentence about `load_knowhow`, or delete it when it only described the old tool |

**A hit in `data/triggers/<slug>/trigger.toml` is not a file edit.** That file
is a copy of the trigger's `run`, written from its events. Rewrite the
`run.intent` with one `triggers(action="update")` call per trigger, per
`system-knowhow/triggers.md`. The engine then rewrites the file.

Verify by re-running the `retired-tools` scan. It should fall to zero.

User-only message delivery intents and transport receipts are internal bookkeeping. Thread deletion removes intents and cascades receipts with notifications, serialized with recovery.
