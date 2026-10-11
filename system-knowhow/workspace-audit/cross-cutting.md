---
name: Workspace audit: cross-cutting
description: The cross-cutting audit section: credentials in workspace code, broken references, duplicates, removed CLI flags and removed thread-summary fields.
---

# Workspace audit section: cross-cutting

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan removed-flags "spawn-thread[^|]*--(parent|cc-model)|run_coding_agent\([^)]*repo[[:space:]]*=|threads (list|count)[^|]*--has-diff" $all
scan removed-fields "coding_agent_(applying|has_diff|proposed|incomplete|requires_restart)" $all
scan cred-env "CRED_[A-Z0-9_]+" --exclude-dir=auth $all
scan auth-header "Authorization|X-API-Key" $inc apps
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
