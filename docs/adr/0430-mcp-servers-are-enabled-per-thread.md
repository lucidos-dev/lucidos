# 0430: An MCP server is enabled per thread, and running stays a workspace state

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0088](0088-tool-schemas-are-gated-by-capability-not-trimmed.md)
  part 2, and [0195](0195-mid-turn-tool-surface-change.md). Part 4 of 0088
  stands.

## Context

A running MCP server offered its tool schemas to every turn of every thread in
the workspace. `McpManager` held one map of running servers and one generation
counter, and every running turn re-sent its MCP slice when that counter moved
(ADR 0195). So a start in one thread reached every thread.

Measured in one workspace over 7 days, 54% of all agent calls carried Slack's
15 schemas in threads that never used it. One trigger used it. The bill was
about $10.80 a week, but three costs weigh more:

- **Context.** 18.1 million tokens of context went to tools nobody called.
- **Tool choice.** Fifteen extra schemas sat in every chat's choice set.
- **Consent.** A trigger thread approved every MCP call silently, whatever its
  purpose.

The full measurement and the options are in
`docs/plans/2026-10-11-mcp-servers-are-enabled-per-thread.md`.

## Decision

A running MCP server's tool schemas reach only the threads that enabled it.

1. **Running is a workspace state, enabled is a thread state.** The process is
   shared. The schemas ride only on requests of threads that enabled the
   server.
2. **A thread enables a server when it needs it.** It reads one line per
   configured server in its own message tier. Enabling starts the server if
   needed. A trigger can list servers so its thread has them from round 1.
3. **The MCP slice is a function of the running servers and the thread's
   enabled set.** Engine-authored families stay a function of workspace
   configuration only, as ADR 0088 part 2 decided.
4. **ADR 0195's per-round refresh is keyed per thread.** A moved generation
   makes each turn recompute its own slice, and only a changed slice rewrites
   the array. A start in one thread no longer reaches another.
5. **The deferred form is the named survivor.** Declaring every server's tools
   with `defer_loading`, surfaced per thread by `tool_addition`, would keep one
   array per workspace. It is deferred until a measurement shows deferred
   definitions bill nothing and the system-tier loss justifies a second wire
   form.
6. **A trigger's MCP calls need the trigger's listing.** A trigger thread calls
   a server silently only when its definition lists the server, or the server
   has auto-approve set. Otherwise the call is refused and the user is
   notified.

Enabled is durable: it is stored as thread events and projected, so it
survives a restart. Running is not: turn setup starts every enabled server
that is not running. A stop switches the server off in every thread.

## Rationale

- **The cost was in threads that never used the server.** In practice the
  enabled set of a thread is nearly always empty.
- **Threads with different slices still share the engine-tools head.** A
  `cache_control` marker looks back up to 20 positions. A thread's
  `tools[-1]` marker sits 15 schemas past the engine head, so it still reads
  the entry that a thread without MCP wrote there. Threads split only the
  system tier, and only by enabled set.
- **Enabling is a capability change, decided once per thread.** It is not
  disclosure, so ADR 0088 part 4's per-fetch arithmetic does not apply. The
  agent enables a server from a one-line list. It never fetches schemas to
  name a tool.
- **The workspace-wide flips stop.** About 61 turns a week started Slack
  mid-turn, and each one rewrote the tools tier of every running thread. A
  start now moves only the array of the thread that asked.
- **Consent belongs to the consumer.** `docs/taxonomy.md` puts a need on the
  thing that has it. A trigger that lists a server has said it needs it. An
  unlisted call in an unattended thread was a silent grant nobody made.

## Consequences

- ADR 0088 part 2 now reads: engine-authored families are a function of
  workspace configuration, and the MCP slice is also a function of the
  thread's enabled set.
- ADR 0195 keeps its mechanism. The generation now asks "might my slice have
  moved", and a turn whose slice did not move pays nothing.
- A thread that enables a server pays one tools-tier write, once, in its own
  thread. A trigger that lists its servers pays nothing mid-turn.
- Dispatch refuses a tool of a server the calling thread has not enabled, so
  the array still says what dispatch allows.
- A trigger that called an MCP server without listing it now fails loud, with a
  notification, until its definition lists the server. ADR 0214's release
  notice plus audit check carries that migration.
- The MCP budget (`MCP_BUDGET_DIVISOR`) now applies to the servers one thread
  enabled.

## Alternatives considered

**Idle-stop of servers a trigger started.** Rejected. Every start and every
stop changes the workspace array, so each one is a cache flip in every active
thread: about 238 flips a week here. Between start and stop the server still
rides every thread, and a server a chat started is never stopped at all.

**A per-server "offer to" setting.** "All threads" is the old behaviour.
"Threads that enable it" is this decision. "Named triggers" puts the
consumer's need on the server. So enablement plus a trigger field covers it,
and an "offer in every thread" switch can come later if users ask.

**Status quo plus docs.** A doc can tell the agent to stop a server after use.
That turns into idle-stop, and it breaks a concurrent thread that uses the
server.

**The deferred wire form now.** It keeps one array per workspace and full
cross-thread warmth. It runs only on some Anthropic models, behind a beta, and
whether deferred definitions bill tokens is unmeasured. Over the portable form
it saves at most $4.80 a week in the measured workspace. That does not yet pay
for a second wire form. Kept as the named survivor in part 5.
