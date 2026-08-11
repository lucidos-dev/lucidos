# 0061: The embedding-model cache is shared per user, not per workspace

- **Status**: Accepted
- **Date**: 2026-08-11

## Context

The embedding model is a ~465 MB download from the Hugging Face model hub, and it
is byte-identical for every workspace: nothing about it depends on the workspace
it serves. Until now the gateway pinned each spawned engine's
`FASTEMBED_CACHE_DIR` to `<workspace>/.lucidos/fastembed`, so every workspace on a
machine downloaded and kept its own copy. A six-workspace dev machine carried
3.2 GB of the same model, and the pin also discarded the shared value the
packaged app (`<app-data>/fastembed`) and the headless service
(`<data>/fastembed`) already set for the gateway.

Sharing one cache is not free, though. `hf-hub` holds an exclusive `flock` on the
blob for the whole download and a waiter gives up after five one-second tries, so
several engines starting cold at once produce one winner and a set of losers that
fail within seconds. Those failures used to read as fetch failures, which meant a
backoff of minutes and, after three attempts, a notification telling each user
memory was degraded while the download proceeded normally one process over.

## Decision

One model cache per user (or per install), inherited rather than assigned. The
gateway sets no cache directory when spawning an engine; the engine resolves
`HF_HOME`, then `FASTEMBED_CACHE_DIR`, then a shared default of
`${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed`, and applies that default by
SETTING the environment variable. With no per-user cache root at all (no `HOME`,
no `XDG_CACHE_HOME`) it falls back to the old per-workspace path rather than to
fastembed's CWD-relative `.fastembed_cache`: the engine's working directory IS
the workspace, and a workspace gitignores `.lucidos/` but not
`.fastembed_cache/`, so the library default would leave a multi-hundred-MB
directory untracked in the user's own repo.

Lock contention is modelled as an outcome (`CacheOutcome::PeerDownloading`)
rather than an error: the loader waits a few seconds and looks again, without
advancing its backoff schedule or its degraded-notification counter.

Existing installs migrate themselves. The first engine to boot after the upgrade
moves its own leftover per-workspace copy into the shared location if nothing is
there yet, and every engine deletes its own leftover once the model has
demonstrably loaded from somewhere else.

## Rationale

The cache is a cache: it is content-addressed by repo and etag, it is rebuildable
from the network, and no part of it is workspace state. A per-workspace copy
bought isolation nobody needed and charged 465 MB per workspace for it. The pin
was also not even reliable isolation, because `HF_HOME` outranks
`FASTEMBED_CACHE_DIR`, so any user with that variable set was already sharing one
cache across every workspace.

**Applying the default by setting the variable is load-bearing, not a style
choice.** `fastembed` reads `FASTEMBED_CACHE_DIR` itself inside
`InitOptions::new`, while the engine pre-fetches the files through `hf-hub`
directly (that is how the download reports byte progress at all). The two halves
must resolve the same directory or the model downloads twice, so a default only
the engine's own resolver knew about would defeat the change it was making.

**Inheritance, rather than the engine choosing a path outright**, is what keeps
the packaged install coherent: the model stays under app-data, so uninstalling
the app takes it with it, and a user who has pointed `HF_HOME` at their own hub
cache keeps sharing with their other tooling.

**Treating a peer's lock as an outcome** follows from the same reading. On a
shared cache "somebody else is fetching this right now" is an ordinary state of
the world, not a fault, and the honest response is to wait a moment rather than
to retreat into a failure schedule designed for an offline machine.

## Consequences

- One copy of the model per user, and a workspace directory that no longer
  carries a multi-hundred-MB cache. `.lucidos/` goes back to being small and
  genuinely rebuildable.
- A parallel cold start downloads once. The engines that lose the lock poll the
  shared cache every few seconds and come online moments after the winner, with
  no notification and no backoff.
- Deleting a workspace no longer reclaims 465 MB, because the cache is not the
  workspace's to reclaim. It is owned by the install (packaged and service) or by
  the user's cache directory (dev), and an uninstall removes the former.
- Upgrading an existing install costs no download: one workspace's copy becomes
  the shared cache and the rest are deleted. The two helpers that do this are
  registered in `docs/temporary-measures.md` and come out once no supported
  upgrade path can still be carrying a per-workspace copy.
- Nothing changes about memory itself: this is disk only. Every running engine
  still builds its own ONNX session, so N engines hold N copies of the model in
  RAM. Sharing that would need an embedding service the engines call.

## Alternatives considered

**Keep a copy per workspace.** The status quo. Isolation with no isolation
requirement behind it, at 465 MB each, and it silently overrode the shared
directory the packaged surfaces had already chosen.

**Share the directory but leave lock contention classified as a fetch failure.**
Correct in the end (the losers do converge on the 30s/60s/120s schedule), but it
sends a false "memory is degraded" notification per workspace on a first boot,
which is precisely when a new user is least able to tell a real problem from a
non-problem.

**A cross-process lock of our own, so only one engine even attempts the
download.** More machinery for the same result: `hf-hub` already serializes on
the blob, and the losers need a wait-and-look-again loop either way. The lock
would add a failure mode (a stale lock file after a crash) that the current
design does not have.

**Hardcode the shared path in the engine, ignoring the environment.** Would break
the packaged install (the model would land outside app-data and survive
uninstall) and would take away a user's ability to point at their own `HF_HOME`.

**Copy the tree instead of moving it during the upgrade migration.** A copy would
survive a cross-filesystem workspace, but it doubles the disk high-water mark at
exactly the moment we are trying to reclaim disk, and it has to reproduce the
`hf-hub` blob-and-symlink layout by hand. A `rename` moves the tree intact and
atomically; the rare cross-filesystem case falls back to an ordinary download.

**A one-off cleanup script the user runs.** Most users would never find it, and
the ones who did would be deleting multi-hundred-MB directories on our say-so.
The engine can prove the shared copy works (it just loaded the model from it)
before removing anything, which no script can do as safely.
