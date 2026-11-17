# 0677: Tool grants live in the permissions store, imported once, and the three grant files become its mirror

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

1. **One import per file.** On first read, each lane's file is imported as one
   `PermissionGrantsChanged` with source `legacy-import`. A unique index makes
   the import event its own stamp, so two boots cannot import twice. A missing
   file imports the compiled defaults, which are empty. Old coding-agent thread
   allows import as thread grants.
2. **Each line keeps its meaning.** A row feeds the lane's own matcher the same
   line the file did, so no grant widens and none is dropped. A line the
   matcher does not recognize is kept and honoured as before, flagged
   *legacy unmapped*, and gets one Keep or Revoke card.
3. **The file is a write-through mirror.** Every standing change writes the
   mirror before it emits, under one lock. A change whose mirror cannot be
   written is refused, and a failed emit restores the old mirror.
4. **A hand edit grants nothing.** The engine reconciles the mirror with the
   rows at boot, before each standing change, and at any gate read that sees
   them differ. A deleted line is revoked (`file-edit`), since deleting only
   takes power away. An added line raises one review card in the home thread
   and drops out of the mirror. It grants only when the owner taps Import
   (`file-review`). A missing mirror is rewritten.
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

Keeping the files as a mirror keeps a downgrade honest. An older engine reads
the file and finds exactly the live grants.

Refusing hand-added lines closes the self-grant path without breaking a user
who edits the file. Their line is not lost: it waits on a card for the owner.

## Consequences

- The Settings allowlist editors replace the lane's standing set. Their GET
  returns the mirror, so a comment typed into the editor does not come back.
- `--allowedTools` is built from the rows at each spawn, never from the file.
- Every lane's boot sweep closes an open card with one reason,
  `Interrupted by an engine restart before anyone answered`. The card reads it
  as "Closed when Lucidos restarted. Nobody denied it."
- An e2e test that sets grants writes them through the settings endpoint. A
  file write is a hand edit and grants nothing.

## Alternatives considered

- **Keep the files authoritative and log writes.** Rejected: a log cannot stop
  a write, so the self-grant path stays open.
- **Delete the files after import.** Rejected: a downgrade would then find no
  grants and ask for everything again.
- **Translate each line into a new matcher.** Rejected: any translation risks
  widening a grant. The line keeps its exact meaning under its own lane's
  matcher instead.
- **Honour hand-added lines with a warning.** Rejected: that is the self-grant
  path with a notice attached.
