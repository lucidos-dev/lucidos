# 0815: Cloud backup retention and latest status count only archives from this workspace copy

- **Status**: Accepted
- **Date**: 2026-11-25
- **Related**: [0520](0520-backups-seal-to-an-age-recipient.md), [0779](0779-backup-readiness-and-archives.md)

## Context

A restored workspace can inherit the source's cloud backup destination. If it
has the same workspace name, both copies see the same archives. Retention used
the name alone, so a backup from one copy could delete archives of the other.

## Decision

Each workspace copy stores a random 12-character lowercase hex backup origin
in its database, bound to its canonical workspace directory. Cloud backup
filenames end in `-{timestamp}-o{origin}.age` or `-{timestamp}-o{origin}.enc`,
depending on the sealing key. Retention and Settings' latest cloud backup
require a matching workspace name and origin. Unmarked archives remain
available to restore.

## Rationale

The filename works with each provider's ordinary list response. The origin
stays stable across engine restarts. Archive restore clears the source origin
in the database restore transaction. In-place snapshot restore carries forward
the current copy's origin through replay. The path binding also detects a copied
database at a different path. Backup can start uploading only after it resolves
its origin.

## Consequences

Existing unmarked archives count toward neither retention nor latest cloud
backup. Copies on older software can still prune unmarked backups by name.
A manual database copy at the same canonical path retains the source origin
unless it goes through Lucidos archive restore. The engine and the gateway use
one parser for both generations of archive name.

## Alternatives considered

- Installation identity would rotate when a workspace moves to another gateway
  and would couple backup ownership to how Lucidos is installed.
- Provider metadata would require provider-specific support. Dropbox does not
  expose inexpensive custom metadata in its normal list response.
- Pausing scheduled backup or asking on restore would interrupt backup coverage.
  Origin-scoped retention lets both copies continue on their existing schedule.
- Adopting unmarked files via `BackupCompleted` would copy ownership across a
  restore because the database dump includes the source's events.
