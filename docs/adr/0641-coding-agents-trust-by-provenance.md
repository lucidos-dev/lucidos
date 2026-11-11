# 0641: Coding agents trust by provenance: every session takes the workspace grants, and a trigger needs its coding-agent grant to start one; supersedes ADR 0125, 0628 and 0630 for coding agents

- **Status**: Accepted
- **Date**: 2026-11-09

## Context

Two rules raised permission cards that no grant could ever silence.

1. **Attend mode.** `cc_permission::resolve_attend_mode` asked "is a human
   present?". An engine-raised thread (`MainCheckRed`, `MainCheckFlaky`, an
   agent-queued `MergeConflict` lane) was unattended, so the workspace grants
   did not apply (ADR 0125). Every refused shape then parked an "Allow once"
   card (ADR 0628, ADR 0630). The owner answered ten such cards in one
   `/harden` run, and the thread stayed unattended throughout.
2. **Protected paths.** A `Bash` command that may write under `.claude/` or
   `.git/` carded in every thread, with an empty grant offer (ADR 0634,
   ADR 0638). Even a bare `Bash` grant could not cover it.

Presence is the wrong axis. Watching a thread does not make its agent safer.
What matters is who caused the work and what input steers it.

Under a bare `Bash` grant, the protected-path rule is no boundary either. The
agent can build the path at runtime. Claude Code's own docs say its path
protections do not limit shell commands.

## Decision

1. **Every coding-agent session takes the workspace's grants**, whoever started
   it: a device, the engine, a lane, the CLI, an API caller, or a granted
   trigger. Attend mode, the unattended lane, the timed park and the
   *permission wait* are gone from the coding-agent lane.
2. **The trust decision sits at spawn.** `agent_session::spawn_gate` walks a
   thread's provenance to the work's cause before any run starts, resume
   included. A run whose cause is a trigger starts only when that trigger holds
   its *coding-agent grant*. Otherwise the turn ends with `ResponseFailed`, and
   the trigger's failure notification names the trigger and the missing grant.
3. A cause of `Webhook`, `System`, or an `Agent` with no thread to trace never
   starts a coding-agent run. None of them is a trigger to grant. Neither
   does a walk whose read fails: an unknown never authorizes.
4. **Every trigger that fed the work needs the grant**, not only the root's.
   A shared continue key lets one trigger's fire into another trigger's
   thread, and that input steers the thread's agents as much as the root.
5. **The walk reads authenticated provenance only.** A verified origin token
   outranks the `caller_workspace` and `caller_thread_id` body fields, which
   anyone can write. A scheduled script's token names its trigger and no
   thread, so its `Api` origin carries `emitting_trigger_id`.
6. **Only the owner sets the coding-agent grant.** The HTTP API refuses `true`
   from an agent caller and from an app frame, unless the trigger holds it.
   The LLM tool cannot set it, and a plugin's shipped `trigger.toml` never
   does. Anyone may revoke it. No trigger gets it automatically: a release
   notice and a workspace-audit check find the triggers that need it.

   An agent's or app frame's edit to a granted trigger's `run`, `on` or
   `mcp_servers` clears the grant. The reply tells the agent to ask the owner
   to grant it again.
7. **Answering a card never changes trust.** It stores exactly the clicked
   pattern, as before.
8. **Protected paths guard narrow grants only.** A bare `Bash` grant covers a
   command that may write under `.claude/` or `.git/`. A narrow
   `Bash(<head>:*)` grant never does. With no covering grant, the card offers
   "Always allow" (`Bash`).
9. The catastrophic floor and the secret-read block stay above every grant.

## Rationale

A workspace grant is the owner's statement about what a coding agent may do in
this workspace. The only question left is whether the work is the owner's. A
person, the CLI, an API caller and the engine on its own work are the owner's.
A trigger acts for the owner only as far as the owner trusts its input, so the
owner grants it per trigger.

The decision belongs at spawn because that is where the cause is known and the
cost of refusing is lowest. Inside a session, a per-request presence check
punished the owner for not watching. It did nothing about the input that
actually steered the agent.

The walk follows provenance, not the callback tree. A top spawn traces back to
the thread that caused it, and so does a child moved to top level. Otherwise a
trigger's run could launder a child into the owner's work.

A narrow grant names one command head the owner looked at. A write under
`.claude/` or `.git/` through that head is something the owner did not agree
to. A bare `Bash` grant already lets the agent write there by building the path
at runtime. Refusing the literal path under it only produced cards.

## Consequences

- Engine-raised threads (main check, lanes, successor sessions) run under the
  workspace grants, so they stop carding for shapes the owner already allowed.
- A trigger that started coding agents stops doing so until the owner ticks
  "Allow coding agents" on it. The run fails loudly rather than skipping.
- A granted trigger's coding agents hold every grant the owner gave coding
  agents. Its intent's input steers them, so the owner grants only a trigger
  whose input they trust.
- The trigger `permission_wait` field and the `unattended_permission_wait`
  preference are removed. An old event or `trigger.toml` naming
  `permission_wait` still parses, with the field ignored.
- A trigger's *side-effect grant* still governs its own chat-lane commands. A
  coding-agent thread it starts does not read it.
- The release notice reads the callback tree only, for speed. A trigger whose
  only coding agents are top spawns learns from the gate's refusal instead.
- A same-workspace `lucidos spawn-thread` from a subprocess now records an
  `Api` origin, not a `Workspace` one. A token holder that sends `mode:
  "engine"` beside it gets a 400, as on every other route. `mode: "human"`
  already got a 403 from the subprocess check.

The boundary covers the paths an agent takes by default. It does not cover a
local process that drops its token: a bare `curl` with `caller_workspace`, or
one presenting the machine-local token, still reads as the owner's. So does a
call from another workspace, whose threads this engine cannot see. An agent
that edits a granted script trigger's script file is outside it too, since
that file is an ordinary workspace file. A trigger that steers such a call is
outside this decision. A follow-up designs an authenticated caller for the
token-less paths, so the gate can stop trusting an unsigned claim.

This supersedes, for coding agents:

- ADR 0125's placement below the unattended fast path. The per-prompt read of
  `cc-allowed-tools`, bounded by `derive_allow_pattern`, stands.
- ADR 0628 in full, and ADR 0630's attend classes and park gate.
- The unattended-lane addenda of ADR 0002 (2026-06-25, 2026-08-24) and the
  presence rule of its 2026-09-28 addendum.

It amends ADR 0634 and ADR 0638: their read-only walk now decides only whether
a narrow grant covers a command.

## Alternatives considered

- **Keep attend mode and widen the attended classes.** ADR 0630 did that once.
  Each new engine reason reopened the question, and presence still decided
  nothing about safety.
- **Let a human's answer to a card attend the thread.** Rejected: a click on
  one card would lend trust nobody chose to give, and it leaves the first card
  in place.
- **Seed the coding-agent grant on triggers that already start coding agents.**
  Rejected: the owner never chose it. The notice and the audit put the choice
  in front of the owner instead.
- **Let an agent set the coding-agent grant.** Rejected: an agent could then
  widen its own trigger's authority.
- **Keep protected paths refused under a bare `Bash` grant.** Rejected: the
  agent bypasses it by building the path at runtime, so it only cost cards.
- **Put the gate in `claim_thread_for_spawn`.** Rejected: the claim runs before
  the spawn message is persisted, so a new thread has no origin to walk yet.

Plan: `docs/plans/2026-11-09-coding-agents-trust-by-provenance.md`.
