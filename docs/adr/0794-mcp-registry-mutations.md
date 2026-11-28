# 0794: The MCP registry announces every mutation itself and releases its guard before dispatch

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0032](0032-a-state-write-owns-its-announcement.md), [0195](0195-mid-turn-tool-surface-change.md), [0430](0430-mcp-servers-are-enabled-per-thread.md)

## Context

`crates/lucidos-engine/src/mcp/mod.rs` keeps the registry of MCP servers.
Registering, starting, stopping and removing a server changes the agent's tool
surface. The same facts feed the request path, the cost report and the settings
page, and a turn holds a tool surface for many rounds.

This record restates the design comments of
`crates/lucidos-engine/src/mcp/mod.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- Registry mutations announce through a held announcer, not one passed per
  call. `McpServerStore`'s mutators require it, so a caller cannot skip it. See
  `core::announced_surfaces`.
- One function resolves a tool against everything that decides whether a
  request carries it. The request path and the cost report both use it.
- A status list is rolled up beside the per-server figures, so the header and
  the rows read the same data.
- A tool surface carries the time it was true. A turn can then check whether
  what it holds is still current. See `McpManager::tool_surface`.
- A dispatch takes only immutable handles and two display values off the
  registry, so the guard is released before any wait on the server. Whether a
  tool is switched off is read live at dispatch.
- A status distinguishes a tool list from `Cache` and from `NeverObserved`. A
  server nobody has connected to is never reported as zero cost.
- Stop and Remove share one helper that takes a server out of the registry and
  records that the surface moved. They differ only in what they do with the
  row.
- Starting a server validates its id again, and caches the manifest only after
  `connect` returns. A cache refresh emits no event, and the row's
  `tools_observed_at` stamp is the freshness signal.

## Rationale

- Announcing is required by the store, so changing the tool surface cannot
  happen silently.
- Two functions answering the same question separately is how a header total
  starts disagreeing with what was sent. One resolver and one rollup prevent
  that.
- Holding the guard across a server wait would block the registry. A snapshot
  of immutable handles avoids that. A live read of the off switch keeps
  dispatch current.
- An empty manifest for an unobserved server states something the engine does
  not know. `NeverObserved` says so honestly.
- A stored row is not proof that its id passed validation. Only a running
  server advertises tools, so the check at start keeps every advertised tool
  dispatchable. An unusable id would give the model tools that no call can
  reach.
- Caching after `connect` means a failed start reports the last good manifest
  instead of nothing.
- A cache refresh is an observation, not a surface change, so it announces
  nothing.

## Consequences

- Every new registry mutator must go through the announcing path.
- A new consumer of a server's tools must use the shared tool resolver, not
  compute its own answer.
- A failed start still shows the last good tool list, with its observed stamp.
- A server that has never connected shows no tools and is labelled
  `NeverObserved`.
- A held tool surface can be compared with its stamp to decide whether to
  refresh it.

## Alternatives considered

- Passing the announcer per call was rejected, because a caller could then skip
  it.
- Having the request path and the cost report each decide what a server
  contributes was rejected, because their totals can drift apart.
- Reporting an empty manifest as a zero-cost server was rejected, because it
  claims knowledge the engine lacks.
- Letting an unusable id run was rejected, because it gives the model tools no
  call can reach.
- Keeping the whole `RunningServer` in the dispatch snapshot was rejected in
  favour of immutable handles and two display values.
