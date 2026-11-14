# 0456: Every directory Lucidos stores data in is a storage root kept at 0700 by whoever opens it, with no process umask, and Lucidos temp files go through tempfile.

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The threat is another OS user on the same Mac or Linux host, through fast user
switching or a shared machine. A process of the same user is not the threat.

Lucidos created its storage under the default umask, so directories came out
0755 and files 0644. On macOS a home is 0750 with group `staff`, and every
standard user is in `staff`, so they can traverse it. Older Linux homes are 0755.
So another user could read:

- a workspace registered outside a private parent, such as `~/workspaces/<name>`
  or an adopted directory: artifacts, knowhow, the git repo, logs, worktrees;
- `~/.lucidos`: the dev gateway's registry and paired devices, the headless
  install's instance data, `network.toml`, the user knowhow repo;
- `~/.cache/lucidos`, whose compile cache holds build output of the user's repos;
- temp files on Linux, where `/tmp` is shared. The legacy migration wrote a
  plaintext `pg_dump` there at 0644;
- agent scratch files, because the coding-agent prompt named `/tmp`, which is
  shared on macOS too.

The 0600 secret files and the 0700 Postgres data directories were already safe.
The packaged app's data dir sits inside `~/Library`, which macOS keeps 0700.

## Decision

A **storage root** is a directory Lucidos stores data in, kept at 0700. The
storage roots are the Lucidos home, the gateway data dir, every workspace root
and the user cache root. Whoever opens one first runs
`lucidos_home::ensure_storage_root`: create at 0700, then an unconditional
`chmod 0700`. Files inside keep the umask's modes.

Lucidos sets no process umask. Its own temp files go through `tempfile`, and
`clippy.toml` bans `std::env::temp_dir`. Agents are steered to
`${TMPDIR:-/tmp}`.

## Rationale

- **A 0700 root closes the whole tree.** The kernel refuses traversal, so the
  mode of anything below stops mattering. Every path that writes into a
  workspace, from git checkouts to the browser profile, is covered without
  touching any of them.
- **Existing installs converge with no sweeper.** Every opener runs the same
  unconditional `chmod`. There is no check of the current mode, so no branch
  detects a bad state and no timer looks for one. A root another user owns
  fails the `chmod`, and the start fails with the owner's uid.
- **A new root is never open, even briefly.** `DirBuilder` creates it at 0700,
  so there is no create-then-chmod window.
- **One definition.** Fourteen Rust sites spelled out `$HOME/.lucidos`. They now
  resolve through `lucidos-home`, a dependency-free crate every binary can take.
  The dev scripts use its shell twin, `scripts/lib/storage_root.sh`.
- **`install.sh` keeps a copy.** A piped install fetches only the libs the site
  publisher serves, so a new lib would break it. A test in `lucidos-home` pins
  the copy's body to the lib's.

## Consequences

- A workspace root, the Lucidos home, a gateway data dir and the user cache root
  read 0700 after any Lucidos start. Adopting a directory makes it 0700.
- A new temp file in Rust goes through `tempfile`. A caller that must name the
  temp dir itself states why in an `#[expect(clippy::disallowed_methods)]`.
- **Known limitation: agent scratch on Linux.** `$TMPDIR` is usually unset
  there, so a steered log still lands in `/tmp`. A private `/tmp` for the
  headless service (`PrivateTmp=yes`) fails to start where unprivileged user
  namespaces are off.
- **Known limitation: unsteered writes.** A file an agent writes to `/tmp` on
  its own initiative is outside every storage root. Only a umask reaches it,
  and a umask is rejected below.
- **A filesystem that refuses mode changes refuses the start.** Examples are
  vfat on Linux and some network shares. A workspace or gateway data dir there
  fails to boot with the path, since Lucidos cannot make it private. The
  optional user cache root is the exception: it is skipped and logged.
- **Out of scope: other products' directories.** Codex writes its session
  rollouts under `~/.codex/sessions` at 0644. Lucidos does not change another
  product's directory.
- Nothing revokes an earlier read. A copy, an open descriptor or a hard link
  made before the `chmod` survives it.

## Alternatives considered

| Alternative | Why not |
|---|---|
| A process umask of 077 | Every child inherits it: git, Claude Code, Codex, build tools. Files an agent creates in the user's own projects come out 0600. A Docker `COPY` into an image that runs as a non-root user then fails, as does a deploy that copies modes. |
| Per-call modes on every file and directory | Hundreds of write paths, many in third-party code (git, Chrome, Postgres). One missed path reopens the hole, and nothing could check it. |
| A sweep that tightens modes on a timer | The bad state would exist between sweeps, and a timer is a recovery path for a state the root makes impossible. |
| Refusing to start on an open root | Every install from before this rule would stop working. The root is ours, so tightening it is safe. |
| Setting `TMPDIR` to a Lucidos directory for every child | Nothing would clean it without a sweeper, and the OS cleans the macOS per-user temp dir already. |
| A new served lib for `install.sh` | The site publisher serves a fixed route set, so the next piped install would `.`-source a web page. |
