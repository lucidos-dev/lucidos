# 0663: Allow metadata on macOS reopened ancestors; scope the sandbox opt-out per workspace

- **Status**: Accepted
- **Date**: 2026-11-11
- **Amends**: ADR 0645

## Context

ADR 0645 hides each gateway data dir except its `workspaces/`
folder. The seatbelt denies `file-read*` on the whole hidden root, the root
folder included. So `lstat` of the root fails, and `realpath(3)` fails on any
path below it. A stock framework Python resolves its venv's `bin/` folder
before it starts, so every sandboxed Python in a gateway-created workspace
failed on macOS.

Decision 8 says the opt-out applies "on this machine". The preference is
stored once per workspace, and Settings writes only the current workspace.

## Decision

1. For each reopened folder, the profile allows `file-read-metadata` as a
   `literal` on every folder between it and its hidden root. The root itself
   is included.
2. Nothing else changes. Listings, contents, extended attributes and socket
   connects inside a hidden root stay denied. A hidden root with no reopened
   folder gets no exception.
3. Linux needs no rule, since Landlock does not mediate path walks or `stat`.
4. `agents_unsandboxed` applies to the workspace it is set in. Decision 8 is
   read that way.

## Rationale

A reopened folder that no tool can resolve is not usable. The
narrowest grant that restores resolution is metadata on the exact folders a
path walk crosses. The names of those folders are already public. Their
timestamps are the only new signal, and it is coarse.

## Consequences

A sandboxed child on macOS can `stat` the hidden folders
above `workspaces/`. It cannot list or read them. An owner who opts out in
one workspace leaves every other workspace sandboxed.

## Alternatives considered

- **Reopen the whole data dir for metadata (`subpath`).** Rejected: it would
  expose the metadata of every secret file inside.
- **Move workspaces out of the gateway data dir.** Rejected here: a storage
  migration for every install, to fix a rendering gap.
- **Tell owners to opt out.** Rejected: ADR 0645 forbids widening the
  boundary to get past one error.
- **Use a non-framework Python.** Rejected: it hides the gap for one tool and
  leaves every other path resolver broken.
