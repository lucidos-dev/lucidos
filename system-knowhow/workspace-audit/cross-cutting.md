---
name: Workspace audit: cross-cutting
description: The cross-cutting audit section: credentials in workspace code, broken references, duplicates, removed CLI flags, removed thread-summary fields and retired LLM tools.
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
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

- **A credential written into workspace code.** One rule over four surfaces, so it lives here, not in the `apps`, `knowhow` and `scripts` sections. Where the workspace calls an external API the engine holds a credential for, the credential belongs in the credential store and the backend in `data/config/apis.json`. Flag:

  - An inline auth header. `curl -H "Authorization: Bearer $CRED_<NAME>"`, a pasted literal token, an `Authorization` / `X-API-Key` / `Bearer` header built in JS, or `requests.get(url, headers={"Authorization": ...})` and its equivalents.
  - Prose telling a future session to "set `$CRED_X`", or to read the credential out of the environment.

  Walk `data/apps/**`, `data/scripts/**`, every `scripts/` under a trigger, and the fenced `bash` / `sh` / `python` / `js` / `ts` blocks in `data/knowhow/**/*.md`.

  The fix differs only by caller: `lucidos.proxy(name).fetch(...)` in an app, `lucidos proxy <name>` in a script, `proxy_request` for the LLM. Severity: **drift**, since the call still works. Give a pasted literal token its own line in the report: the file is git-tracked, so the credential needs rotating, not just rerouting. Owns the rule: `system-knowhow/js-sdk.md` § `lucidos.proxy` and `system-knowhow/lucidos-cli.md` § `lucidos proxy`.

- Broken references: missing `knowhow:` ID, missing script path, manifest pointing at a deleted asset.
- Duplicated content: same knowhow text in two files, same script copied between apps.
- Patterns the source-of-truth files explicitly mark deprecated. Grep for the old form and point at the doc that flags it.
- **Removed CLI flags and tool args still passed by workspace code.** These were
  removed after a deprecation window, and a recipe passing one now fails with a
  rename error. Grep each form across `data/knowhow/**/*.md` fenced code blocks,
  `data/scripts/**`, each trigger's `scripts/`, and `data/apps/**`, and recommend
  the replacement. Severity: **broken** (the call errors on next use). Currently
  removed:
  - `lucidos spawn-thread --parent` → `--relation child` (a same-workspace
    parent-with-callback spawn). Do NOT flag `threads list --parent <uuid>` or
    `threads count --parent <uuid>`, which are current, unrelated filters.
  - `lucidos spawn-thread --cc-model` → `--coding-agent-model` (same value).
  - `repo` passed to the `run_coding_agent` tool → `folder` (which also accepts a
    registered repo name). Do NOT flag the current `lucidos spawn-thread --repo`
    flag, which is a different, live argument.
  - `lucidos threads list --has-diff` and `threads count --has-diff`, and the
    `threads` tool's `has_diff` arg → `--change-state` / `change_state`. The
    tool and the HTTP route refuse a stale `has_diff` by name.
    `--has-diff false` becomes `--change-state none`. `--has-diff` covered both
    `unproposed` and `proposed`, so ask which one the code means. The scan
    finds only the CLI form: read knowhow `threads` calls for `has_diff` by
    hand. Do NOT flag `has_diff` on `CodingAgentDiffChanged`, which is current.
- **Removed thread-summary fields still read by workspace code.** A thread
  summary comes from `lucidos.threads.list`, `lucidos threads list` or the
  `list_threads` tool. A removed field reads as `undefined` in JS, so the code
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
- **Retired LLM tools still named by workspace code.** The `retired-tools`
  scan finds every mention across `data/apps/**`, `data/triggers/**`,
  `data/knowhow/**` and `data/scripts/**`. Fix every hit, so none survives the
  run. Currently retired:
  - `execute_intent`. An intent now runs in the thread that asks for it: load
    it with `load_knowhow(<intent id>)` and do its steps in the same turn.
    Owns the rule: `system-knowhow/intent-registry.md`.
    - **broken**: an instruction to call it, such as "use `execute_intent` to
      run the digest". The call now fails with a hint.
    - **stale**: a passing mention that tells nobody to call it, such as a note
      that an intent "runs through `execute_intent`".

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding, and the root's § Inline or a coding agent picks its
route.

### Rewriting a retired tool call

**Route: inline.** Each row rewrites prose or one call, and changes no control
flow.

| Old | New |
|---|---|
| An instruction to call `execute_intent(intent_id: "<id>")` | `load_knowhow(id: "<id>")`, then do its steps in this turn |
| An instruction to run the work in a separate thread through `execute_intent` | `run_thread`, only where the work truly needs its own thread |
| A passing mention of `execute_intent` | the same sentence about `load_knowhow`, or delete it when it only described the old tool |

**A hit in `data/triggers/<slug>/trigger.toml` is not a file edit.** That file
is a copy of the trigger's `run`, written from its events. Rewrite the
`run.intent` with one `update_trigger` call per trigger, per
`system-knowhow/triggers.md`. The engine then rewrites the file.

Verify by re-running the `retired-tools` scan. It should fall to zero.
