---
name: Workspace audit: cross-cutting
description: The cross-cutting audit section: credentials in workspace code, broken references, duplicates, removed CLI flags, removed thread-summary fields, retired LLM tools and the retired work-item names.
sources: ["system-knowhow/js-sdk.md", "system-knowhow/js-sdk/*.md", "system-knowhow/lucidos-cli.md", "system-knowhow/lucidos-cli/*.md", "system-knowhow/coding-agent-flows.md", "system-knowhow/intent-registry.md", "system-knowhow/triggers.md", "system-knowhow/triggers/*.md", "crates/lucidos-cli/src/*", "crates/lucidos-core/src/llm/tools/*", "crates/lucidos-agents-api/src/engine/tools/*", "crates/lucidos-agents-api/src/api/threads*", "crates/lucidos-core/src/core/store/threads/*"]
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

- **The lander inferred from pairing direction.** Read `lucidos landing setting` in the workspace whose behavior you are auditing.
- **Fix evidence counted as an owner decision.** Check the kind and origin of `PlanApproved`.

- **Standing denies edited through their mirror.** The permissions store
  controls these rules. Flag recipes editing the mirror file as **broken**.
  See the *standing deny* entry in `system-knowhow/glossary.md`.

- **A guard verification failure mistaken for a standing deny.** The hook
  reports when it cannot verify standing denies. Check listener readiness
  before treating that refusal as a stored permission rule.

- **Apply mistaken for a live version.** Check version claims against `system-knowhow/glossary/core-terms-15.md` § Build deferral.
  Flag recipes claiming a held version is running as **stale**.


- **Pairing mistaken for agent return authority.** An outgoing pairing alone never grants the peer access through its caller.
  Check routed recipes against `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § `lucidos workspaces targets` and `system-knowhow/remote-access/settings-access-2.md` § Agent return access.
  Flag credential copying, inferred return grants or remote child callbacks as **broken**.

- **Pairing mistaken for workspace control.** A paired gateway acts on this gateway's workspaces only while its control or admin grant is on.
  Check recipes against `system-knowhow/remote-access/settings-access-2.md` § Control a remote gateway's workspaces.
  Flag recipes that pass `lucidos pair --gateway --control` or `--admin` unasked, or expect a remote delete or rename, as **broken**.

- **Direct spawns assumed to verify the calling workspace.** Check recipes against `system-knowhow/lucidos-cli/subcommands-4.md` § `lucidos spawn-thread`.
  Pair gateways to check future calls; retrying the same direct spawn does not establish its identity.
  Flag a recipe that spawns into a paired gateway's workspace by absolute path, not `--to <workspace>@<pairing>`, as **stale**.

- **Host-log transfers with simulated consent or blind retries.** Flag agent-authored approvals and retries of uncertain operations as **broken**.
  Check `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos diagnostic-log prepare-host-transfer` and § `lucidos diagnostic-log host-transfer-status`.

- **Gateway control requests bypassing consent.** Check agent channel recipes against `system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos update-channel`.
  Flag token reads, caller-selected ports or approval reuse for a different request as **broken**.

- **Browser media helpers used in unattended scripts.** `lucidos.media.poster` needs a mounted app frame and its host bridge.
  Flag scripts or recipes expecting it to work without a browser as **broken**.
  Reference: `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.media`.

- **Ceiling handoffs assumed to need Continue.** Check recipes against `system-knowhow/lucidos-cli/subcommands-12.md` § `lucidos successor start`.
  Interrupted work continues after the agent's narrative turn; natural turn ends and engine briefs wait for input.
  Flag unconditional Continue instructions as **stale**.

- **Ordinary owner questions from integrated sub-threads.** Flag recipes that
  rely on an integrated sub-thread opening an ordinary owner card as **broken**.
  Check `system-knowhow/lucidos-cli/subcommands-7.md` § `lucidos wake-parent yes|no` and `system-knowhow/lucidos-cli/subcommands-11.md` § `lucidos planned mark`.

- **Prepared batches mistaken for visible cards.** Check batch, workspace classify and workspace delete recipes against `system-knowhow/lucidos-cli/subcommands-9.md` § `lucidos permissions batch`.
  Flag ending after preparation without asking the question tool with the printed id alone as **broken**.
  Lucidos runs the approved batch. Flag repeating it after Run all or Allow once as **broken**.

- **Home hold reach confused with resume authority.** Check recipes against `system-knowhow/lucidos-cli/subcommands-3.md` § `lucidos threads resume [--thread <uuid>]`.
  Home may hold any thread in its workspace. Only the owner resumes, or the thread's own agent inside a turn the owner's message started.
  Flag a recipe that has Home, a parent or a trigger run `lucidos threads resume` as **broken**.
  Flag a recipe that still runs `lucidos threads pause-automation` as **stale**: it is a one-release alias of `lucidos threads hold`.

- **Ladder edits assumed open to agents.** Check ladder recipes against `system-knowhow/lucidos-cli/subcommands-4.md` § Editing a ladder.
  An agent's edit needs the owner's `edit-intelligence-ladder` permission.
  Flag a recipe that writes a ladder preference directly, or skips the printed request id, as **broken**.

- **Private paths treated as workspace privacy.** Private paths cover external files and folders.
  Workspace data and registered repository history remain available through Lucidos.
  Check the enforcement level in Settings → Permissions before claiming a process is confined.
- **Home writes mistaken for sandbox probes.** Review probes against `system-knowhow/agent-sandbox.md` § Is this process sandboxed?
  Flag a successful home write treated as proof of an unsandboxed process as **broken**.
  An inherited sandbox protects the outer engine's workspace, so a probe must target that workspace.

- **Changed plans treated as missing approvals.** For a `plan_changed` hold, check whether the agent can restore the approved text.
  Flag a routine re-approval request as **broken** when restoring that text would clear the hold.

- **Successor backend choices contradict the flow.** Check questions against `system-knowhow/coding-agent-flows` § Backend policy belongs to the flow.
  When a new plan approval is required, flag missing fresh-session choices for readable backends as **broken** when handoff is available.
  At other checkpoints, flag mandatory alternatives based only on installed backends as **broken**.
  Check intelligence fit before backend preference and keep backend, model and effort flags together.
  Compare estimates only against equal, present references.
  Check generated model fallbacks against `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § Common keys before proposing changes to stored model pins.

- **Event query boundaries treated as inclusive.** CLI `events query` and `events count` use exclusive `since` and `until` bounds.
  Audit inventory windows against that contract before asserting event parity.

- **Deferred labs counted as measurements.** Check `status` before comparing lab results.
  Match host fingerprints and scenario settings using `system-knowhow/lucidos-cli/scoreboard-lab-results`.

- **Restored billing matched across process boundaries.** Check against `system-knowhow/coding-agent-events/billing-evidence.md` § Billing evidence.
  Codex restored usage needs the latest prior main-session capture, an exact token vector and that process's persisted backend-session binding.
  Missing evidence stays unknown; a broader history search cannot supply it.

- **Direct-port device authority.** Check rig recipes against `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § `lucidos workspaces device-session`.
  Flag registration alone or paired device ids in sessions as **broken**.
  Lasting mint consent must bind device, exact TTL and gateway-recorded creator
  workspace; missing/foreign provenance stays once-only. Flag reuse across any
  other device, lifetime or creator, or a mint without current provenance validation, as **broken**.
  Flag prechecks treated as mints or sessions reused after re-keying as **broken**.

- **Trigger runs in Review.** Check for `ThreadReadRequested` before counting a run as requiring a read. Placement in Current and a trigger's call to `request_read` can precede the judge's decision. See [Triggers](../triggers.md).

- **Integrated reports counted as owner work.** Use the thread summary to check whether a read request stands.
  An integrated child's `ThreadReadRequested` with `parent_report` records a decision without adding a request.
  A hardening hold alone routes to the parent. See the glossary entries for *Read decision* and *Blocked*.

- **Thread verdicts from history.** Read current `liveness` before calling a thread stalled, failed or done.
  Use the thread-scoped events header or a thread list/search row; see `system-knowhow/glossary/core-terms-10.md` § Thread liveness.
  Flag recipes that count a held thread as waiting on the owner because its turn status still shows an open question.

- **Red main mistaken for a check stop.** Checks continue while red; continued failures retain one repair owner.
  Check automation against the main repair contract in `system-knowhow/glossary.md`.
  Flag recipes that start a separate repair for every failed or flaky finding as **broken**.

- **Confusing acceptance with message recording.** Ordinary HTTP sends acknowledge durable acceptance, not execution.
  Check retries against `system-knowhow/thread-queue.md`; preserve the original wire intent after an unknown outcome.
  Flag recipes clearing a newer compose draft after replay as **broken**.

- **Fit-only credentials mistaken for owner authority.** Check renderer recipes against `docs/widget-fit-renderer.md`.
  Flag credentials in shell state, frame messages, URLs, saved evidence or general request headers as **broken**.
  Only the selected main-frame fit-report request may carry the credential; it grants no Device or owner authority.

- **Stop restoration mistaken for a response-only draft.** Stop restores unread chat inputs into durable compose with each removal (ADR 0710).
  Reports, reload and replay retain exact restoration additions until the next submission consumes the journal.
  Compose PUTs preserve that journal; pending devices reconcile additions once without replacing local edits.
  Flag recipes that append the Stop response again, or clear the journal on a compose PUT, as **broken**.

- **Saved-draft snapshots mistaken for authority.** Review draft recipes against `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos threads drafts`.
  A snapshot's revision and epoch identify saved content; neither grants permission to submit it.
  Flag direct compose clearing or arbitrary Human-mode POSTs offered as submission shortcuts as **broken**.
  Submission goes through `lucidos threads submit-draft` (§ `lucidos threads submit-draft`), with a standing instruction or a permission.
  Flag a recipe that submits a draft on reach alone, even over its own child, as **broken**.

- **Agent text standing in for the owner's answer.** Check recipes against `system-knowhow/lucidos-cli/subcommands-8.md` § `lucidos permissions list` and `system-knowhow/lucidos-cli/subcommands-3.md` § `lucidos threads resume [--thread <uuid>]`.
  Only the owner's pick on a permission request or a batch grants, and only the owner resumes a thread on hold.
  A trigger's program or folder grant needs the owner's answer to the engine's permission request.
  A trigger intent run that raises a program grant request waits for an answer. Its Allow once covers that run only.
  Only the owner lifts a *trigger deny*, so a recipe that revokes one through the trigger update API is **broken**.
  The owner starts the review of earlier runs with **Check my workspace** in the release notice.
  The review still asks the owner for each trigger's program grants.
  A recipe may tell an agent to grant itself a permission, resume a thread on hold, or send a draft.
  Flag it as **broken** when it lacks the owner's authority.

- **Assuming every announced artifact becomes memory.** Check recipes against `docs/system-events.md` § Artifact provenance and provider holds.
  Unattended trigger writes stay readable but do not create workspace memory leaves.
  Flag a recipe relying on automatic recall of those files as **drift**; have it read the file explicitly.

- **Pairing links in recipes.** Generate fresh device links with `lucidos pair`; never copy credentials into query URLs or workspace files.

- **An ambiguous mutation result treated as a refusal.** Check the response for the
  `workspace_repo_busy` reason or the boot rejection marker.
  Use `system-knowhow/lucidos-cli/subcommands.md` § `lucidos data write` for repository refusals.
  Use `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § Waiting through an engine restart for transport and boot outcomes.

- **Cleanup authority.** Workspace review is read-only. Refuse recipes that use local tokens for manual deletion or infer ownership from workspace names.
  Keep the gate’s reason when reporting a delete or bulk archive refusal. A device lookup failure calls for a retry.
  For agent archive, check the caller's subtree, the owner's standing instruction, or a permission for the target.
  Flag recipes that treat archive authority as permission to bypass lifecycle or pinned-thread checks as **broken**.

- **Restore authority inferred from a caller-provided workspace.** Review snapshot recipes against `system-knowhow/backups/workspace-snapshots.md` and `system-knowhow/lucidos-cli.md`.
  Flag self-restore instructions or retries after unknown delivery as **broken**.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

- **A credential written into workspace code.** One rule over four surfaces, so it lives here, not in the `apps`, `knowhow` and `scripts` sections. Where the workspace calls an external API the engine holds a credential for, use its builtin proxy or configure a credential-backed entry in `data/config/apis.json`.
  Builtin sign-ins stay inside the engine; see `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`. Flag:

  - An inline auth header. `curl -H "Authorization: Bearer $CRED_<NAME>"`, a pasted literal token, an `Authorization` / `X-API-Key` / `Bearer` header built in JS, or `requests.get(url, headers={"Authorization": ...})` and its equivalents.
  - Prose telling a future session to "set `$CRED_X`", or to read the credential out of the environment.

  Walk `data/apps/**`, `data/scripts/**`, every `scripts/` under a trigger, and the fenced `bash` / `sh` / `python` / `js` / `ts` blocks in `data/knowhow/**/*.md`.

  The fix differs only by caller: `lucidos.proxy(name).fetch(...)` in an app, `lucidos proxy <name>` in a script, `proxy_request` for the LLM. Severity: **drift**, since the call still works. Give a pasted literal token its own line in the report: the file is git-tracked, so the credential needs rotating, not just rerouting. Owns the rule: `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy` and `system-knowhow/lucidos-cli/subcommands-13.md` § `lucidos proxy`.

- Broken references: missing `knowhow:` ID, missing script path, manifest pointing at a deleted asset.

- **Build slot grants reused across jobs.** A launch build owns its grant until the build shell releases it or dies.
  Flag recipes that persist the grant's environment for reuse by other jobs as **broken**.
  A queued background build has yet to receive a grant.
  See `system-knowhow/lucidos-cli/subcommands-6.md` § `lucidos build-slot --lease`.
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
  - Removed `lucidos update-address` → `lucidos update-channel`; use `add <name> <url> --use`, `set stable`, or `show`.
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
- **Relay openings counted as a thread's own card history.** `SubThreadCardOpened`
  carries routing metadata in the `sub_thread_card` aggregate. Read the source
  thread's events to audit the original card and its resolution.
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
  `system-knowhow/js-sdk/lucidos-threads-thread-management.md` § "lucidos.threads.users". A hit in the Factory
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

## Diagnostic evidence

Run `lucidos diagnostic-log query` for runtime evidence and check the reported status of each requested source.
See [Agent Sandbox](../agent-sandbox.md) for the access rules.
A maintainer bundle omits legacy log text; its absence does not establish an audit finding.

- **Frontend previews through remote gateways.** Check instructions against `system-knowhow/lucidos-cli/subcommands-12.md` § `lucidos frontend-preview`.
  Flag instructions bypassing trusted pairing or copying device credentials as **broken**.
  The preview uses the opening gateway origin and requires no extra exposed port.
