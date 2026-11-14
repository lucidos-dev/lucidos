# 0645: Agent subprocesses run in an OS sandbox that denies the private subtree, and the sockets carry their own credentials

- **Status**: Accepted
- **Date**: 2026-11-10
- **Amends**: ADR 0516 (its key-read rule becomes an early refusal, not the boundary)

## Context

Every subprocess Lucidos starts for an agent runs as the owner's OS user. That
covers chat bash and python, scripts, Claude Code, Codex, background tasks and
user MCP servers. Steered by outside input, any of them could read the local
token, the shared Postgres password and the secret keys. It could also connect
to the bundled Postgres socket as superuser, or to an engine socket as the
gateway. It could rewrite the grant files in `<workspace>/.lucidos/`.

ADR 0516's command guard matched literal paths only, and scheduled scripts
skipped it. Its temporary-measures row named an OS sandbox as the real
boundary.

Plan: `docs/plans/2026-11-10-os-sandbox-for-agent-subprocesses.md`.

## Decision

1. **Every secret and control file lives under `~/.lucidos/private/`.** The
   sandbox denies read, write and unix-socket connect anywhere inside it. A new
   secret written there is covered with no list to update.
2. **macOS uses a seatbelt** through `/usr/bin/sandbox-exec`, with a profile
   Lucidos writes. It allows by default and denies the private subtree.
3. **Linux uses Landlock**, applied in the child before `exec`. It grants every
   sibling of `private/` along the path from `/`.
4. **Writes to `<workspace>/.lucidos/` are denied** except under `worktrees/`.
5. **The sockets carry their own credentials** on every platform. The bundled
   Postgres uses `scram-sha-256` for the superuser instead of `peer`. The
   engine socket demands a credential from `private/`.
6. **Coverage is everything that runs agent-steered code.** That is chat bash
   and python, `pip install`, and scheduled, trigger and proxy handshake
   scripts. It is also Claude Code, Codex, background tasks and user MCP
   servers. The frontend preview's Vite is covered too, since an agent may
   write its config.
7. **The sandbox wraps the whole agent process.** Codex runs with its own
   sandbox off, on macOS and Linux alike, and asks for every command and file
   change (`approvalPolicy: untrusted`). So it runs like Claude Code: each
   command raises a card, which coding-agent auto-answer can settle, and a
   write outside the worktree raises a card. Only the opt-in `exec` protocol, which has no card channel,
   keeps a write allowlist.
8. **A sandbox that cannot apply fails closed.** One owner preference, "Run
   agents unsandboxed on this machine", lifts that, with a standing health
   warning.
9. **The local-token switch derives from the sandbox state**, never from a
   separate toggle. The switch is on only while
   `runtime::agent_sandbox::status` returns `Enforced`. Every other state
   leaves it off, `Inherited` included, since an inherited sandbox guards
   another home. The status reads the owner preference on each call, so an
   opt-out turns the switch off with no restart.
10. **The command guard's key-read rule stays**, repointed at `private/`, as an
    early and friendlier refusal.
11. **Linux keeps the strict Landlock shape.** A sandboxed child cannot create,
    delete or swap an entry directly in `$HOME` or `~/.lucidos`. Every existing
    subfolder stays fully writable.
12. **An older home keeps its entries in place.** A home an older release used
    keeps its secrets and control files directly in `~/.lucidos`, and every new
    binary resolves them there. The sandbox denies that closed list of old
    names beside `private/`. Nothing moves or links them.

## Rationale

**One subtree beats a list.** A named deny list goes stale the day someone adds
a secret file. A wholesale deny on one directory covers the next secret too.

**The sockets cannot lean on the sandbox.** Landlock on the floor kernels
(Ubuntu 22.04 runs 5.15) does not gate `connect()` to a pathname unix socket.
A credential on the socket closes that leak everywhere, sandboxed or not. On
macOS the seatbelt also refuses the connect, as a second layer.

**A seatbelt does not nest.** Phase 0 measured it on macOS 27.2: under an outer
profile with any `deny` rule, an inner `sandbox_apply` fails with EPERM. Codex
and Claude Code apply their seatbelts per command, so theirs cannot run inside
ours. Wrapping the whole agent is still required, because Claude Code's `Read`
tool reads in-process.

**Codex's Linux sandbox does not nest either.** Codex 0.167 sandboxes through
bubblewrap, which needs mount calls. Landlock refuses those to a restricted
process, so bubblewrap fails inside ours ("Failed to make / slave"). Measured
in a Docker Linux guest at Phase 7.

**Landlock needs no privilege.** It works without user namespaces, setuid
helpers or packages, and inside containers. Every floor distro (Ubuntu 22.04,
Debian 12, Fedora 36, RHEL 10) enables it in its default LSM list. The engine
still probes the ABI at runtime and never infers support from a kernel
version.

**No secret travels in an environment.** On macOS any same-user process reads
another's environment through `KERN_PROCARGS2`, and no seatbelt rule stops
it. So the engine and the gateway's client tools get a password file's path,
which the sandbox denies, and never the password.

**Shipped binaries cannot be patched.** An engine re-adopted across an upgrade
(ADR 0461), or a dev build from an older branch, opens the old paths. A move
that left links broke both: the engine socket guard refuses a linked `run`,
and the `network.toml` writer renames a file over its link. Resolving each
entry where an older binary also looks needs neither.

**Fail closed keeps the promise honest.** A sandbox that silently disappears
is worse than none, because the owner believes it is there.

## Consequences

- An agent can no longer read or touch Lucidos's own secrets. The rest of
  `$HOME` stays readable: `~/.ssh`, cloud CLI credentials and browser profiles
  are not covered, and the network stays open.
- A tool that calls `sandbox-exec` from inside a covered process fails on
  macOS. Known cases: `codex` run as a tool, and SwiftPM manifest builds.
  The confinement tests hit the same wall. The engine runs them outside the
  sandbox under an owner permission ([ADR 0690](0690-confinement-tests-run-outside-the-agent-sandbox.md)).
- On Linux below Landlock ABI 3 (before 6.2, so Ubuntu 22.04 and Debian 12
  stock kernels) the sandbox cannot stop truncation. An agent there can empty a
  protected file, though it still cannot read, change or replace one. Agents
  still run, and Settings → Permissions → Agent sandbox says so.
- The frontend preview refuses when the gateway's own certificate lies in a
  hidden folder, since the sandboxed Vite cannot read its key. Nothing
  Lucidos sets up puts one there, and the owner chose to keep the key hidden.
- On Linux an agent cannot add or remove an item at the top of `$HOME`. An
  unpinned Linux Claude Code session therefore runs with `CLAUDE_CONFIG_DIR` at
  `~/.claude`. Lucidos creates `~/.cache`, `~/.config` and `~/.local` ahead of
  time.
- Owner control-plane CLI acts (`lucidos pair`, `tls`, `external-address`)
  fail inside a sandbox, by design.
- Dev scripts that read the shared Postgres superuser password move to a
  narrower `CREATEDB` role.

## Alternatives considered

- **bubblewrap or a mount namespace on Linux.** It hides `private/` cleanly and
  frees the top of `$HOME`. Rejected: it needs unprivileged user namespaces,
  which Ubuntu 24.04 restricts by default and many containers block. Reconsidered
  for decision 11 and rejected again by the owner.
- **Create or remove rights on `$HOME` under Landlock.** It frees the top of
  `$HOME`, but Landlock has no subtraction. The same right reaches `private/`,
  so an agent could delete the secret keys or forge the local token.
- **A named deny list** of secret files. Rejected: every new secret needs a
  list update, and a missed one leaks silently.
- **Deny all of `$HOME` with carve-outs.** It would cover `~/.ssh` too, but it
  breaks git over SSH and every cloud CLI. A separate, larger trade.
- **Move every entry into `private/` and link the old paths.** Tried in the
  first build and backed out. The links broke the shipped socket guard and
  `network.toml` writer above. The move also ran from any read of a token, so
  tests moved the developer's real home. A later release may
  move the entries with no links, once no binary older than decision 12 can
  run (`docs/temporary-measures.md`).
- **A separate OS user for agents.** ADR 0516 named it. It needs privileges a
  one-click install does not have.
- **Nesting Codex's own seatbelt inside ours.** Measured, and macOS refuses it.

## Note: `ps` under the macOS sandbox

macOS runs `ps` and `top` setuid, and the seatbelt refuses a setuid binary, so
an agent could not list processes. The engine writes a `ps` shim to
`.lucidos/sandbox-bin` and puts it on a sandboxed child's `PATH`. It runs
`lucidos ps`, which reads `kern.proc.all` and never guesses what the kernel
withholds. The dir stays read-only to agents, and Linux and the owner's opt-out
keep the system `ps`.

A macOS login shell runs `path_helper`, which puts `/bin` ahead of the shim.
Codex runs every command as `zsh -lc`, so while the shim is on `PATH` the
engine passes Codex `allow_login_shell=false`. The stand-in also refuses `-E`.
It would print a same-user process's environment, such as the engine's.

Rejected: a `pgrep` stand-in (it is not setuid and works)
and a `top` stand-in (`ps -o pid,%cpu,rss,command` covers it). Also rejected:
a `ZDOTDIR` that re-prefixes `PATH` after `path_helper`, which would replace
the user's own zsh startup files. The plan is
`docs/plans/2026-11-11-agent-ps-in-the-macos-sandbox.md`.
