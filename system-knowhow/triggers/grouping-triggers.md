---
name: Triggers, Grouping triggers
description: Part of Triggers. Its index lists the sections it covers.
---

One part of `system-knowhow/triggers`, which lists every part.

## Grouping triggers

A *trigger group* is a user-visible folder, shown as a collapsible section in the triggers panel. Groups are pure labels: no schedule, no code, no coordinated firing. They only collect related triggers under one header so the panel stays readable.

Use a group when several triggers form one workflow and the user benefits from seeing them together. For example, one emits an event via `events(action="emit")` and another subscribes through its `on` field. A single trigger needs no group; the "Ungrouped" section at the bottom of the panel holds it.

| Tool | When to use |
|---|---|
| `trigger_groups(action="list")` | Before assigning a trigger, check whether a fitting group already exists. |
| `trigger_groups(action="create", name, order?)` | Create a new section header. Names are unique within the workspace (case-insensitive). |
| `triggers(action="create")` / `triggers(action="update")` with `group_id` | Assign a trigger to (or move it between / out of) a group. `triggers(action="update", group_id: null)` clears membership. |
| `trigger_groups(action="rename", group_id, name)` | Rename the section. |
| `trigger_groups(action="reorder", [{id, order}, ...])` | Batch-reorder panel sections. |
| `trigger_groups(action="delete", group_id)` | Refused if the group still has members; move or delete them first (the error response lists them). |

Groups are orthogonal to `app_id`: an app-owned trigger can live in any group. `app_id` drives notification deep-linking, `group_id` drives panel layout.

## Program grants

With command safety on, a trigger's intent can run an irreversible command when its program grant covers the invocation. The grant belongs to that trigger. A standing deny still takes precedence.

A program grant cannot cover a command the workspace floor cannot rule out as
destructive. The run fails and reports the reason. See
`system-knowhow/running-python/pick-the-right-tool.md` § The command guard (dangerous commands are gated).

A command that needs a grant pauses an intent run. The run's thread waits on a permission request, and a notification links it. The run has not failed, and its conversation stays as it was. The owner picks one of four answers:

| Answer | What happens |
|---|---|
| **Allow once** | The run carries on in the same thread. The grant holds for this run only, so the next fire asks again. |
| **Always allow** | The grant joins the trigger's grants, and the run carries on in the same thread. |
| **Always deny** | The entry joins the trigger's denies, and the run ends. Later runs that need it end at once, with no card. |
| **Don't allow** | The run ends as refused, not failed. The trigger's grants stay as they are. |

A restart while the resumed run is working drops an Allow once grant. The run then asks again.

The paused fire counts as run, so the scheduler never fires that slot again. Its last run reads as failed until the resumed run ends and records its own result. An Allow that can no longer apply, for example because the trigger changed after the request went up, ends the run as failed.

A script run has already ended when its command is blocked, since a script has no conversation to resume. Its request offers **Grant and run again**, which adds the grant and queues the fire again, **Always deny**, or **Dismiss**.

Bash grants use the named program scope. A Python grant covers the trigger's Python commands. The grant list lives in the trigger's settings, where **Delete** on a row revokes that grant.

A **folder grant** allows a trigger's sandboxed subprocesses to write in one folder.
A write outside the trigger's folders asks the same way: an intent run waits on the four answers above, and a script run offers to grant and run again.
Each launch checks that the grant still names the approved folder. Static scope checks come first.

Trigger details lists each folder grant beside the program grants, and **Delete** on a row revokes that one grant.
Only the owner can revoke a folder grant. Through the trigger update API, send `revoke_folder_grants` with the listed folders.

Ask for grants when you create or edit a trigger, before its first run needs them.
Pass `folder_grants` and `program_grants` as lists of absolute folder paths and
program names on the trigger create or update call. Lucidos posts one permission
card in the thread doing the work and saves only the listed access on approval.
A folder path must name an existing directory. Lucidos refuses relative paths,
paths with `..`, the filesystem root and the home directory itself.

For an existing trigger, you can also ask in advance with
`lucidos ask-permission grant-trigger-folders --trigger <id> --folder <path> --reason "<why>"`
or `grant-trigger-programs` with `--program <name>`. Repeat a flag to include
more than one entry, then use the printed request ID with your question tool.
A card asked ahead offers **Always allow**, **Always deny** and **Cancel**. No run
waits on it, so it has no Allow once, and Cancel saves nothing.

### Trigger denies

**Always deny** on any trigger card saves a *trigger deny*: that trigger may
never use the program or folder. It holds for this trigger only. Other threads
and triggers still ask, and the workspace-wide refusal stays the command card's
Always deny.

- A run that reaches a denied entry ends at once, with no card. Its thread says
  which deny stopped it.
- A program deny matches its exact pattern. A folder deny also covers the
  folders inside it.
- An entry is never both granted and denied. Saving a deny removes a matching
  grant, and saving a grant removes a matching deny. Folders conflict when one
  holds the other, since a wider grant would let the denied writes through.
- Trigger details lists the denies under **Denied**. Only the owner can lift
  one: through the trigger update API, send `revoke_program_denies` or
  `revoke_folder_denies` with the listed entries. An agent cannot.

The additional write restriction applies under macOS Seatbelt and Linux Landlock ABI 3 or later.
On macOS it also refuses metadata changes such as chmod, chown, touch and setxattr outside the scope.
On Linux with unprivileged user namespaces, the trigger child sees everything outside the scope
mounted read-only, so those metadata changes fail too.
Where the kernel refuses the namespace, Linux runs the trigger under Landlock alone.
Metadata changes then stay open, and Settings → Permissions → Agent sandbox shows a note.

A hard link gap remains on both systems: a file in a writable folder that shares its contents with
an outside name still accepts writes. Sandbox off, inherited mode and earlier Landlock retain the
static scope check. A refusal a script hides can stay silent.

Category grants ended on upgrade. In an affected workspace, **Check my workspace** includes
“Help me review my trigger grants” in its Lucidos Agent thread. When you receive that request, use `lucidos triggers list` to find triggers with a nonempty `side_effect_grant`.
Prepare a `grant-trigger-programs` request for one of those triggers at a time. When the user asks to review refused
folder writes, prepare `grant-trigger-folders` in the same way.

Use `lucidos ask-permission <action> --trigger <id> --reason "<why>"`, then ask your question
tool with the returned id alone. With no explicit folders or programs, the engine fills the card from recorded runs. If the engine
finds no grants to add, move on to the next trigger. Wait for the user's answer before preparing
the next card and answer any questions they ask. An approval saves only the grants on that card
for future runs. The review does not run the trigger.

To revoke program scopes through the trigger update API, send `revoke_program_grants` with the listed patterns.

A coding-agent thread needs the trigger's coding-agent grant to start. It uses the workspace's coding-agent permissions for its own commands.

### Coding-agent grant

A trigger's runs may start a *coding-agent thread* only when the trigger holds the **coding-agent grant** (`coding_agent_grant`, default `false`). It applies to intent and script triggers alike, since a script can start a thread through the CLI. Without it, the spawn is refused and the run fails with a notification naming the trigger and the missing grant.

The engine traces each coding-agent run to its cause. A child, a top-level spawn or a CLI spawn from a trigger's run all count as that trigger's, however many threads lie between. Moving a thread to top level does not change its cause. A thread a webhook started never starts a coding agent, and neither does anything it spawns.

A trigger whose fire continues another trigger's thread (a shared continue key) needs the grant too, since its input steers that thread's agents. Once such a trigger is deleted, nobody can grant it, so that thread starts no more coding agents: archive it.

**An agent's edit to the work clears the trigger's grants.** The user granted the trigger as it was. So when an agent or an app frame changes a granted trigger's `run`, `on` or `mcp_servers`, the edit lands. The coding-agent grant and program grants drop, and the reply says so. **Tell the user plainly** that the grants are gone, and ask them to grant what the new version needs if they trust it. A rename, a pause or the user's own edit keeps them.

**Only the user grants it.** The user ticks *Allow coding agents* in the trigger's settings. The `triggers` LLM tool cannot set it. The HTTP API, CLI and SDK refuse `true` from an agent or an app frame when the trigger lacks it, and accept `false` from anyone. A plugin's `trigger.toml` never sets it, and a batch's `trigger_grant` item asks the user for coding-agent access. So when a trigger's intent needs a coding agent, **tell the user** to tick it, and say what steers the trigger: its coding agents hold every grant the user gave coding agents.

**An *unattested caller* gets a 403 when it tries to widen a grant or change the work a granted trigger runs.** A script holding only the machine-local token is unattested. Use the Lucidos app, or use a trigger so the job runs as an agent.

### Context ceiling

A trigger's optional `context_ceiling` is copied onto each thread it fires, as a `ContextCeilingSet` (ADR 0629). Its coding-agent sub-threads inherit it at spawn. The value is `off`, a size such as `200k`, or a window share such as `30%`. A trigger without one leaves its threads on the workspace preference `coding_agent_context_ceiling` (default `300k`). An agent may set a trigger's ceiling no higher than the workspace's; only the user raises one or turns it off.

## MCP servers the fires use: `mcp_servers`

A running *MCP server* offers its tools only to the threads that enabled it (ADR 0430). A trigger lists the servers its intent needs in `mcp_servers`, for example `["slack"]`. Each fire then enables them in its thread and starts any that are not running, before the first step. So the tools are there from round 1. A listed id that no server has becomes a line in the fire's `[MCP SERVERS]` block, and so does a server that fails to start. The run still goes ahead.

**List every server the intent needs.** Two things happen to an unlisted one:

- **Its calls are refused.** A trigger thread has nobody to approve an MCP call. So it runs one silently only when the trigger lists the server, or the server has auto-approve set. Any other call is refused, and the agent is told not to call that server again in this fire. The user gets one notification naming the trigger and the server, which opens the trigger.
- **Enabling it costs a mid-turn write.** The fire's agent can still `mcp enable` an unlisted server, but that rewrites its tool array mid-turn, and the calls are refused anyway.

Do not list a server the intent does not need. Its tools then ride every request of every fire for nothing. The `triggers` tool sets the field on `create` and `update`, and so do `lucidos triggers` and the HTTP API. Each write is a full replacement, and `null` or `[]` clears it. A trigger that calls no MCP server leaves it out.

The audit's `triggers` section flags a trigger whose fires enabled a server it does not list. Its `prompt-footprint` section flags one that lists a server its fires never call.

## Edit, don't recreate

**Always look for an existing trigger first** (`triggers(action="list")`) and change it with `triggers(action="update")`. Call `triggers(action="create")` only when no comparable trigger exists. Recreating gives the new trigger a fresh `trigger_id` and orphans the old one's run history. The threads still exist, but no longer match the live trigger in the filter dropdown, trigger-scoped reports, or anything else that joins by id. The user sees no threads for a workflow that has fired for months.

This applies to every shape of change:

| User says | What to do |
|---|---|
| "Change the cron to 9am" | `triggers(action="update", trigger_id, cron=...)` |
| "Rename it to X" | `triggers(action="update", trigger_id, name="X")` |
| "Switch it to fire on event Y instead" | `triggers(action="update", trigger_id, cron=null, on=[{event_type:"Y"}])` |
| "Also fire when Z happens" | `triggers(action="update", trigger_id, on=[existing..., {event_type:"Z"}])`: append to the `on` array, don't make a sibling trigger |
| "Stop firing on event Y" | `triggers(action="update", trigger_id, on=[existing... minus Y])`: `on` is a full replacement |
| "Tighten the Y filter" | `triggers(action="update", trigger_id, on=[..., {event_type:"Y", condition:{...}}, ...])`: replace that entry inside the full list |
| "Tweak the prompt" | `triggers(action="update", trigger_id, run={...})` |
| "Pause it" | `triggers(action="pause", trigger_id)` (or `triggers(action="update", ..., paused=true)`) |
| "Make sure I see this one" / "Send to review" | `triggers(action="update", trigger_id, go_to_review=true)` |
| "Stop bringing this up, keep it in the archive" | `triggers(action="update", trigger_id, go_to_review=false)` |
| "Add another time it should run" | `triggers(action="update", trigger_id, cron=[existing..., new_expr])`: append to the cron array, don't make a sibling trigger |
| "Run it once more, like at 7pm tonight" | `triggers(action="update", trigger_id, cron=[existing..., one_shot_expr])`, then another `triggers(action="update")` after it fires to remove the one-shot entry. Never a duplicate trigger, not even temporarily |

**An edit, pause or delete never stops a run in flight.** A trigger write re-arms or disarms the schedule only. A fire that is already running finishes on its own, and the new configuration applies to the next fire. The user stops a live run from its thread (ADR 0650). A scheduled fire still waiting in the queue when the write lands is dropped. A queued event fire or catch-up fire stays queued and still runs.

If you truly need a different trigger (a different *workflow*, not a tweak), give it a clearly different name. Two live triggers with the same name are a UX trap: no picker tells them apart.

## Know-how a plugin removed

A plugin update or uninstall can delete knowhow a trigger relies on. The trigger
is then an *affected trigger*. The usual case is one an agent made with
`triggers(action="create")` during a plugin's setup: it carries no plugin id, so the plugin
cannot update it. The engine never edits, pauses or deletes it. It warns instead:
in the plugin panel, on its receipt, and in one notification
(`system-knowhow/plugins/three-distribution-shapes.md` § "Affected triggers").

**Each later fire carries a `[REMOVED KNOW-HOW]` block** in its first message:

```
[REMOVED KNOW-HOW]
A plugin removed know-how this trigger relied on:
- `apps/token-cost/knowhow/daily-price-check.md`, removed by the Token Cost update to v0.6.0
Do not invent a replacement recipe silently. ...
[END REMOVED KNOW-HOW]
```

When your fire carries it, do what it says. Look for current knowhow with
`load_knowhow`, and say in your result that this trigger's knowhow was removed.
Suggest the user edit the trigger. The run still goes ahead.

The block stops when either holds:

- **The trigger is edited after the removal**, with `triggers(action="update")`. Pause,
  resume and a plugin re-registering its own trigger do not count, because
  nobody looked at the knowhow.
- **Every listed path exists again**, for example after a reinstall.

To fix one, find where the recipe went (the new version may have moved it), and
point the intent at it with `triggers(action="update")`. Never recreate the trigger (see
"Edit, don't recreate" above). A removal from before Lucidos recorded affected
triggers raises no block. The workspace audit checks for those
(`system-knowhow/workspace-audit/triggers.md`).

