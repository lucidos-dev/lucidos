# 0677: Tool grants live in the permissions store, and legacy grant files are imported then deleted

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: ADR 0095 (where a grant is read from), ADR 0664 (grant files stay where they are)

## Context

ADR 0664 made the three tool permission cards permission requests, but left
their grants in three plain files under `.lucidos/`: `agent-allowed-commands`,
`cc-allowed-tools` and `mcp-allowed-tools`. Every lane read its file on every
prompt. Any process that could write a file could grant itself power. Nothing
recorded who added a line. A thread delete could not tell a grant made in that
thread from one that stands.

Thread allows were worse. A command or MCP "Allow for this thread" lived only
in memory, so a restart dropped it. The coding-agent lane rebuilt its own from
the thread's events on first use.

## Decision

**The `permissions` store is the one source of tool grants.** A standing grant
is a row held by the workspace. A thread allow is a row held by the thread. Each
row keeps the exact `pattern` line, its `source`, and the thread it was granted
in.

1. **Import at boot.** Each workspace imports existing grant files, including
   `agent-denied-commands`. The event and its permission rows commit together.
   The existing `legacy-import` unique index stamps each lane, even when no
   file exists.
   Old coding-agent thread allows import as thread grants.
2. **Each line keeps its meaning.** A row feeds the lane's matcher the same
   pattern the file did. A line with an unrecognized shape remains visible as
   *legacy unmapped* in Permissions.
3. **Delete after commit.** If the engine stops between commit and delete, it
   sees the stamp and removes the file on the next boot. A file that appears
   after the stamp is also removed without changing grants. Boot closes open
   and queued legacy review cards.
4. **Settings and cards use the store.** The owner manages standing grants
   under **Settings → Permissions**, or remembers an approval on a tool card.
5. **Thread allows survive a restart** in all three lanes, because they are
   rows.
6. **A thread delete takes its thread allows with it.** Standing grants survive
   it, with no thread recorded.
7. **A card offers only what it can honour.** The engine stamps each card's
   offer, the command card included, keeping only patterns that cover the
   command. A secret read offers no grant and says why.
8. **Trigger grants are not migrated.** Their digest and reach do not change.

## Rationale

A grant is a decision with an author. Storing it as an event-backed row gives
it an actor, a source and an audit trail. It also lets every surface (Settings,
the Permissions page, voice, batches) read the same answer.

Removing the files gives each workspace one place to manage grants. An older
engine that reads legacy files will ask again after a downgrade.

## Consequences

- The Settings allowlist editors replace the lane's standing set. Their GET
  formats the stored patterns as one line per pattern. Editor comments are not stored.
- `--allowedTools` is built from the rows at each spawn, never from the file.
- Every lane's boot sweep closes an open card with one reason,
  `Interrupted by an engine restart before anyone answered`. The card reads it
  as "Closed when Lucidos restarted. Nobody denied it."
- An e2e test that sets grants writes them through the settings endpoint. The
  engine reads legacy grant files only during boot.

## Alternatives considered

- **Keep the files authoritative and log writes.** Rejected: a log cannot stop
  a write, so the self-grant path stays open.
- **Keep the files as mirrors.** This was the original choice for downgrade
  compatibility. The approved engine cards and trust design chose deletion
  and retired file reconciliation and import review cards.
- **Translate each line into a new matcher.** Rejected: any translation risks
  widening a grant. The line keeps its exact meaning under its own lane's
  matcher instead.
