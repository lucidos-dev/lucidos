# 0612: Local workspace snapshots and gateway restore

- **Status**: Accepted
- **Amends**:
  - 0192: a snapshot restore is a second sanctioned rewrite of the event log.
    It archives the discarded timeline in a recovery snapshot and deletes
    nothing.
  - 0015: restore into a NEW workspace stays in the picker. Restore of the SAME
    workspace from a local snapshot is a new engine operation.
- **Relates to**: 0002 (command checkpoints), 0153 (backup exclusions), 0387
  (owner approvals), 0481 (agent DB handle), 0509 (sealed secrets), 0516
  (key reads ask), 0520 (age backup recipient).

## Context

Lucidos has three ways to go back in time, and none restores a whole workspace
in place:

- **Backups** (0509, 0520) build a complete archive: a `pg_dump`, a tar of the
  workspace including `.git` and `data/blobs/`, and the secrets key. They seal
  it to the owner's age recipient and upload it to a cloud provider.
- **Picker restore** (0015) opens such an archive with the owner's backup key.
  It always provisions a new workspace.
- **Command checkpoints** (0002) undo one command's file edits with git. They
  never touch the database.

A user who wants an undo before a risky agent run has no tool. The Demo Director
rig snapshotted its demo workspace with a direct `pg_dump` and `pg_restore`.
That broke when stored secrets moved to sealed columns (0509). The agent DB
handle (0481) cannot read secret columns, so a script cannot take a complete
dump. Reading the secrets key from the Keychain or the key file is not a fix,
and 0516 makes such a read ask.

The engine already owns every piece a local snapshot needs: the engine DB role,
the bundled PG client tools, the archive builder, and the key store. The gateway
already owns the only process that can stop an engine, change its files, and
start it again.

## Decision

1. **A workspace snapshot is an engine-owned, same-machine copy of one
   workspace.** It holds a `pg_dump` of the workspace database and an archive
   of the workspace files. The file scope is the backup scope: everything under
   the workspace root except `.lucidos/`, legacy `data/postgres*`, and
   `data/.backupignore` patterns. It includes `.git` and `data/blobs/`.
2. **A snapshot never holds a key.** It records the ids of the secrets keys its
   sealed values need, not the key. Restore refuses before any mutation when
   the key store cannot open those ids. So a snapshot on disk exposes only
   ciphertext for secret columns, the same as the live database files.
3. **Snapshots live outside the workspace directory**, under
   `<lucidos home>/snapshots/<database name>/`, owner-only. The command guard
   treats that path like the key store (0516): an agent command that reads or
   writes it asks first.
4. **A snapshot is named by an opaque handle** (`snap_<ULID>`). The handle
   resolves only inside the calling workspace's snapshot root. Its manifest
   records identity, component hashes and sizes, schema version, consistency
   facts, and per-table row counts. A key in the key store,
   `<database name>.snapshot`, authenticates the manifest with HMAC-SHA256.
   That key never leaves the engine.
5. **The snapshot registry is the filesystem, never the workspace database.**
   A restore rolls the database back. A registry inside it would forget every
   snapshot taken after the restore point. Restore records go in a journal file
   beside the snapshots for the same reason.
6. **Restore replaces the same workspace in place, and the gateway runs it.**
   The engine checks the caller, the policy and the snapshot. It writes a
   journal entry and asks the gateway. The gateway puts the workspace in
   maintenance and stops the engine. It runs an offline engine subcommand,
   then starts the engine again. The workspace keeps its id, name, port and
   database.
7. **Every restore first takes and verifies a recovery snapshot.** The engine
   is stopped at that moment, so the recovery snapshot is fully consistent. A
   restore that fails after any mutation rolls back to it. A restore that
   succeeds offers "Undo restore", which restores the recovery snapshot.
8. **Restore never replays work from either timeline.** After a restore, the
   engine settles every in-flight row from the snapshot with a new
   non-transient `AbortCause::SnapshotRestored`. It skips trigger catch-up and
   cut-fire retries for slots before the restore. It expires every owner
   approval created before the restore.
9. **Restore is destructive and asks.** A device caller restores after a
   confirmation that names the workspace. An agent caller needs a spent-once
   owner approval (0387). The owner can set a per-workspace policy from a
   registered device that lets agents restore without a card. That policy
   never covers an agent restoring its own workspace. The agent DB role
   cannot write that policy or the restore boundary (0481).

   Like every refusal of an agent caller, this stops an agent using the
   credentials it was handed. An agent set on impersonating the user can
   still read the machine-local token or type a device id on loopback (0192).
10. **Agents reach snapshots through the `lucidos snapshot` CLI**, never a
    chat tool. Every chat agent already has the CLI through `run_bash` with
    its own origin token. A tool schema would repeat it and bill every turn
    of every user. The knowhow sits under `system-knowhow/backups/`,
    where it costs no routing line.
11. **A snapshot is not a backup.** It lives on the same disk as the workspace,
    so it does not protect against disk loss. The UI and the knowhow say so.

## Consequences

- The event log of a restored workspace is a prefix of an earlier log, not the
  log the user last saw. That is a deliberate rewrite, sanctioned here beside
  0192. The discarded suffix survives in the recovery snapshot until the user
  deletes it or retention prunes it.
- Snapshots cost disk. A compressed archive is about one third of the
  workspace size. Retention and a free-space floor bound the cost, and kept
  snapshots are never pruned automatically.
- External side effects are not undone: sent mail, push notifications, remote
  git pushes, calls to third-party APIs. Restore says so before it runs.
- Rows that grant access to the workspace carry forward across a restore, so a
  device paired after the snapshot keeps its access. The agent restore policy
  carries forward the same way, so restoring an old snapshot cannot bring back
  an archived `allow`.
- A workspace with no gateway, such as a bare engine in a test, can create and
  check snapshots but cannot restore one.
- Backups and snapshots share the archive builder and one archive-job slot per
  workspace, so a backup and a snapshot never run at once.

## Rejected alternatives

- **Give scripts the engine DB role or the secrets key.** That reverses 0481
  and 0509 for every workspace to fix one rig.
- **A local backup provider.** A backup is sealed to the owner's age key, and
  the engine holds no private key (0520). Restore would then need the owner to
  paste the backup key every time. An unattended undo is impossible that way.
- **Restore by deleting and recreating the workspace.** That changes the
  workspace id, database name and key name, and breaks every reference to the
  workspace.
- **Restore inside the running engine.** The engine cannot replace its own
  `.git`, files and database under live threads, triggers and an open pool.
  The gateway already owns stop and start.
- **A snapshot registry table in the workspace database.** Restore rolls it
  back, as decision 5 explains.
- **Put the secrets key in the snapshot.** A same-machine copy does not need
  it, and holding it beside the dump undoes sealing at rest for every snapshot
  on disk.
