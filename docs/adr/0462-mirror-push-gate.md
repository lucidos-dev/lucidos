# 0462: The public mirror has one door: release_tree_push pushes by explicit URL, only a scanned commit with empty or published ancestry, and a refusing push URL makes a bare push fail

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

Everything outside `docs/plans/**` and `WORKSPACES.md` reaches the public
mirror, so a push there is the last point where private data can be stopped
(`.claude/rules/no-private-data.md`). Until now four places pushed to it, each
with its own `git push lucidos …`:

| Pusher | Ref |
|---|---|
| `release.sh` | `rc/<version>`, and its deletion |
| `release-to-lucidos.sh` | `main`, and the `v<version>` tag |
| `release_abandon.sh` | deletion of `rc/<version>` |
| `e2e_github.sh` | `e2e/<run-id>`, and its deletion |

Since ADR 0386 e2e runs on GitHub by default, so the e2e pusher runs many
times a day.

Each caller scanned its tree with `release_tree_scan` and checked its ancestry
before pushing. Nothing made a fifth caller do the same. A new script, or a
hand-typed `git push lucidos`, could publish an unscanned tree or private
history.

The landing-lane plan (`docs/plans/2026-10-12-landing-lane.md`, decision 9)
adds an automatic pusher: the main check pushes a stripped copy of `main`
to an `e2e/` branch after every landing. An unattended pusher raises the cost
of a missed check, so the gate ships before it.

## Decision

1. **One function writes to the mirror.** `release_tree_push <repo> <remote>
   <commit> <ref> [--force | --lease=<sha>]` in `scripts/lib/release_tree.sh`
   refuses, naming the rule, unless all three hold:
   - the ref is `main`, `rc/*`, `e2e/*`, or a `v*` tag;
   - the commit's tree passes `release_tree_scan`;
   - its ancestry is published. The commit is the mirror's `main`, or a single
     child of it. A parentless commit passes too, except for `main` once it
     exists. A tag must name the mirror's `main`.

   `release_tree_delete_ref` deletes only `rc/*` and `e2e/*` branches.
2. **Both push by explicit URL**, never to the remote's name.
   `release_mirror_push_urls` is the one list: a real push URL on the remote,
   else the push URLs the gate saved, else `git remote get-url <remote>`.
3. **The mirror is read where the push lands.** The door reads each push URL's
   `main` and checks the commit's ancestry against every one. It refuses if any
   check fails. Every other mirror read (`main`, the tags, the rc branch, the
   e2e sweep, the fetches) goes through `release_mirror_ls_remote` or
   `release_mirror_fetch`, which query the same list. Two push URLs that answer
   one read differently fail it, since the mirror then has no single state.
4. **A refusing push URL closes the side door.** `release_mirror_gate` sets
   `remote.lucidos.pushurl` to `release-tree-push-only::refused`. git reads that
   as a remote helper that does not exist, so a push by remote name fails. Both
   functions run it before every push, and it is idempotent. A real push URL it
   replaces moves to `release-tree.lucidos.pushurl`, and the door pushes there.
5. **A deterministic check keeps it one door.** `scripts/check-mirror-pushers.sh`
   runs in `/harden` Phase 4.5. It fails when a tracked script, workflow or
   Rust source pushes outside a short allowlist, or names the mirror's git URL.
   The allowlist is `release_tree.sh`, the `origin` pushes, and test files.

Pushes to `origin` stay as they are: `release_main_sync.sh`, the
`$ORIGIN_REMOTE` lines in `release.sh`, and the engine's `push_main_in_background`.

## Rationale

**The check belongs where the bytes leave.** Each caller already scanned, but a
caller-side check protects only the callers someone remembered to write it
into. Putting the scan and the ancestry rule inside the push makes them a
property of the mirror, not of each script.

**The ancestry rule is ADR 0039's, widened for run branches.** A push sends
every reachable object, while the scan reads only the tip tree. So the commit's
history must already be public: the mirror's `main`, or nothing at all. A
parentless `e2e/` commit carries only its scanned tree. A parentless `main`
would replace the published history, so ADR 0039 still refuses it there. A tag
anywhere but `main` would break `release_mirror_tags_are_on_main`.

**The push URL turns a convention into a refusal.** The scan inside
`release_tree_push` does nothing for a push that never calls it. With the
refusing push URL in place, a bare `git push lucidos` fails at git itself. The
value lives in the shared repository config, so every worktree gets it.

**The check catches what the push URL cannot.** A script could push by URL and
skip the gate. `check-mirror-pushers.sh` fails `/harden` on such a script
before it lands. Its allowlist stays short, so each entry stays reviewable.

## Consequences

- A new mirror pusher, such as the Phase 4 main check, calls
  `release_tree_push`. It gets the scan and the ancestry rule for free.
- The scan now runs twice on the release and e2e paths: once in the caller,
  once in the gate. That costs seconds. The callers keep theirs, since it
  refuses before slower work starts.
- `scripts/mirror-push-gate.sh` installs the gate, and `/harden` Phase 4.5 runs
  it. So the refusing push URL is in place from a machine's first hardening
  run, before any push. Every gated push re-asserts it.
- A push by explicit URL updates no `refs/remotes/lucidos/*` tracking ref.
  Nothing in the release scripts reads one.
- A `gh` call can still create a ref on the mirror, for example a published
  Release creates its tag. Such refs name objects already on the mirror, so
  they carry no new tree. The check does not cover them.
- **A configured push URL survives the gate** (a call made after review). A
  checkout that pushed over SSH while fetching over HTTPS kept its push URL
  only until the first gated push erased it. The gate now saves a real one
  first, and a push URL set later replaces the saved one.
- **Reads follow the push URL** (a call made after review). Once the push and
  fetch URLs could differ, the door checked ancestry against the fetch
  repository and pushed to another. History the push repository never
  published could then pass the check. So the check, the release lease, the
  history check and the parent all read the push repository now.

## Alternatives considered

- **Keep the scan in each caller, and review new callers by hand.** That is
  the state this replaces. It fails silently the first time a reviewer misses
  a new push.
- **A `pre-push` hook that scans.** Hooks are per clone and skippable with
  `--no-verify`. Codex runs no hooks. A hook would also need the ancestry rule
  and the denylist outside the release libraries.
- **Remove the `lucidos` remote and keep the URL only in the release lib.**
  The read paths (`ls-remote`, `fetch`, `gh` slugs) use the remote by name, and
  `e2e_github.sh` checks for it. Removing it would touch every reader for no
  gain over a refusing push URL.
- **A GitHub branch-protection rule.** It cannot run our denylist, and it binds
  only refs that a rule names. The mirror needs the check on our side of the
  push, before any object leaves this machine.
