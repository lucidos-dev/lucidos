# 0822: Prepare workspace auto-commits outside the repo lock

- **Status**: Accepted
- **Date**: 2026-11-27

## Context

Concurrent script triggers each ran a workspace auto-commit. Each held the
workspace repo lock while staging all of `data/`. A CLI data write spent most
of its response budget in that queue. The engine committed the write, but the
CLI timed out before it received the response.

libgit2's `add_all` leaves stale stat info in place when a file's content is
unchanged. Later auto-commits re-hash those files under the lock.

## Decision

Refresh stat data in a private index outside the lock. When HEAD still matches
the prepared base, reuse the tracked entries. Under the lock, rescan
`data/`, persist the refreshed index, and commit any tree changes.

PUT and DELETE on the data API wait up to 20 seconds for the lock. On
exhaustion, return HTTP 503 with reason `workspace_repo_busy` before any
mutation. The CLI gives data writes 60 seconds for the response. The SDK
uses the same deadline for writes and deletes.

## Rationale

libgit2 writes the index during stat refresh. A private index lets preparation
refresh entries without touching the workspace index. Reusing tracked entries
lets the locked scan skip hashing files whose metadata still matches.

A merge can advance HEAD before syncing the working tree. The locked rescan
sees the synced tree, filters edits using current attributes, and applies
current ignore rules to new paths. A moved HEAD or a retry stages again from
the current base.

## Consequences

A cancelled preparation never takes the repo lock. Once the locked blocking
task starts, it keeps the guard until it finishes.

A data mutation refused for a busy repository leaves the file untouched. A
CLI response timeout can still mean the write is in progress.

## Alternatives considered

- **Respond before committing.** A crash could leave an acknowledged write
  outside Git history. Success still follows the commit.
- **Drop the lock for data writes.** A merge could overwrite a file between
  the write and its commit.
- **Retry automatically in the CLI.** An explicit busy refusal is safe to
  retry, but retrying would extend the wait. Retrying a response timeout
  could repeat a write that already committed.
