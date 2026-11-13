---
name: Workspace snapshots
description: Local workspace snapshots: create, list, check, keep, delete, restore the same workspace, recovery snapshots, owner approval, and interrupted restore status. Load before risky workspace changes or an in-place rollback.
---

# Workspace snapshots

A workspace snapshot is a same-machine copy of the workspace database and
included files. It lives outside the workspace. It includes Git history and
blobs, excludes runtime/worktree directories and backup exclusions, and stores
sealed secret values without their keys. It needs the existing key store:
use encrypted [backups](../backups.md) for off-machine recovery.

## Owner controls

Settings → System → Backup contains Workspace snapshots: total size, Create
snapshot, Keep/Unkeep, Restore and Delete. Keep protects against automatic
retention. Restore confirmation names the workspace and explains its limits.
The picker reports Restoring while the gateway owns the operation.

Create is a live capture: database consistency is fixed, while files may change
during capture. The manifest records file-change counts and capture times.
A recovery snapshot is captured and verified with the engine stopped before
restore changes anything.

## Agent and CLI controls

Use the `lucidos snapshot` CLI family through `run_bash`; `lucidos snapshot
--help` lists targeting and command syntax. Commands include create, list, get, check,
delete, status and restore. Target the intended workspace explicitly when
working across workspaces. A handle is opaque; never read or edit snapshot
storage, keys, dumps or journals directly.

An agent restore needs the owner's exact, single-use approval for that target
and handle, or the target owner's allow policy. Policy alone never authorizes
restoring the agent's own workspace. The target must also be a *machine-made
workspace*: the gateway refuses an agent restore into a user's or unknown one
with `snapshot_restore_target_protected`, under either policy. The owner
restores those from the picker. Source identity must be verified through
the gateway before policy or approval is considered. A lost approval-spend or
restore response is uncertain: inspect status, never blindly retry.

CLI restore accepts `--wait` to poll the admitted restore's exact gateway
journal through verified boot completion. A failed status read ends the wait;
continue with `lucidos snapshot restore-status`, never another restore request.

## What restore does

The gateway stops the target engine. It verifies the selected snapshot and its
key/schema compatibility, then captures and verifies a recovery snapshot.
It preserves current access and runtime/worktree exclusions, then replaces the database and included files.
Failure after mutation runs recovery; unresolved recovery
blocks engine startup. Never restore a live workspace through SQL or shell.

If the offline restore and its recovery cannot reach a safe journal phase,
status reports `needs-attention` and the workspace stays blocked. The picker
shows the failure; waiting longer does not authorize another restore request.

Current device access replaces archived access; a revoked device stays revoked
and a newly paired device keeps access. The agent restore policy carries
forward the same way. Pairing and machine-global state stay
in place. Dirty worktrees and preserved device-local originals remain available.

Restored running work, queued work and old approvals do not resume or gain new
authority. Cron catch-up and cut-fire retry exclude archived slots. Device-local
composer text and selections recover into drafts for review, never automatic
send. Uploads, queued sends and uncertain sends remain preserved for explicit
review. The completion notification offers Undo restore using the recovery
snapshot, through the same owner confirmation.

Restore cannot undo sent messages, remote pushes, purchases or other external
side effects. It replaces the same workspace, not a new workspace. Its external
journal and snapshot registry survive replacement, so later snapshots remain
available after restoring an earlier one.

Backups and snapshot jobs share one archive slot. A scheduled backup that
meets a running snapshot job waits for it, and reports a failure if it never
frees.

Default retention keeps five unkept manual and three recovery snapshots.
Kept snapshots, open-restore references and the newest recovery snapshot are
protected. Capture refuses when projected free space would fall below the larger
of 5 GiB and ten percent of the volume.

SnapshotKeepChanged announces a retention-protection change to every connected
client. Capture completion/failure and deletion also refresh the Settings list.
