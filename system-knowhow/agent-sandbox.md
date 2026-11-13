---
name: Agent sandbox
description: What the OS sandbox around agent commands, scripts and coding agents hides or keeps read-only, on macOS and Linux. Load on "Permission denied" or "Operation not permitted" under ~/.lucidos or .lucidos/, a refused spawn, or a failed `sandbox-exec`.
---

# Agent sandbox

Every process an agent steers runs in an OS sandbox (ADR 0645). That covers
your own `run_bash` and `run_python`, scripts, triggers, coding agents,
background tasks, MCP servers and the frontend preview. macOS uses a seatbelt profile, and Linux
uses Landlock. Child processes inherit it, and nothing inside can lift it.

A permission error on one of the paths below is the sandbox working. It is
not a bug to work around. Pick another route, or tell the user what you
cannot do.

## What it denies, on both platforms

| Path | Inside the sandbox |
|---|---|
| `~/.lucidos/private/` | hidden: no read, no write, no listing of its contents |
| A private file an older release kept directly in `~/.lucidos/` (`local-token`, `secret-keys/`, `snapshots/`, `network.toml`, `run/`, `gateway/` and the rest) | hidden, the same way |
| Each gateway's data dir | hidden, except its `workspaces/` folder |
| A workspace's `.lucidos/` | read-only |
| A workspace's `.lucidos/worktrees`, `runtime`, `tmp`, `exhaust` and `staging` | writable |

Everything else stays as reachable as before: the workspace's `data/`, the
rest of `$HOME`, the network and the engine's HTTP API.

Local e2e runs inside the sandbox. The harness keeps its workspaces, secrets
and Postgres under `~/.lucidos/e2e-root`, which no gateway registers.

## What fails because of it

- **Control-plane `lucidos` acts.** A command that needs the full-authority
  local token cannot read it, such as `lucidos workspaces review`. The CLI
  acts that go through the engine as this thread still work, such as
  `lucidos data write`, `lucidos events emit` and `lucidos workspaces list`.
  The `lucidos workspaces` broker verbs work too: the engine asks the gateway
  for you, on the owner's approval per call.
  `lucidos spawn-thread --to <name>` reaches another workspace by name.
  `lucidos snapshot restore` and `restore-status` also work: the origin token
  proves the caller, so they need no local token.
- **Reading the host log.** `gateway.log` sits in the hidden gateway dir, so
  `grep` or `tail` on it fails. The command guard also cards each such read as
  a secret read, with no grant to offer. Read this engine's own lines with
  `lucidos diagnostic-log query` instead. See
  `system-knowhow/lucidos-cli.md` § `lucidos diagnostic-log`.

## Only on macOS

Folders above a reopened folder keep their own metadata readable, including
any hidden root on that path. This lets `realpath` and framework Python
launchers resolve workspace paths. Each grant covers one folder, never its
contents or extended attributes. Listings and hidden files stay denied.
Hidden roots with no reopened folder inside gain no access.

A seatbelt cannot nest under one that denies anything. So nothing inside the
sandbox can run `sandbox-exec`:

- Claude Code's own sandbox (`sandbox.enabled`) is forced off for the
  session, whatever the user settings say.
- `codex` run as a tool must pass `--sandbox danger-full-access`.
- A SwiftPM manifest build needs `--disable-sandbox`.

Playwright's headless Chromium still runs. Browser e2e is not affected.

### `ps` and `top`

macOS ships both setuid, and the sandbox refuses a setuid binary
(`ps: Operation not permitted`). So the engine puts a `ps` first on `PATH`
that runs `lucidos ps`, a stand-in over the kernel's process table. It takes
the options agents type (`ps aux`, `ps -ef`, `ps -o pid,ppid,command -p <pid>`).
It never guesses: where macOS hides a fact from an unprivileged reader, such as
another user's command line, it prints `(comm)`, `0` or `?`. An unsupported
option exits 2. `system-knowhow/lucidos-cli.md` § `lucidos ps` lists what it
supports.

- `pgrep` and `lsof` are not setuid and work unchanged.
- There is no `top`. Use `ps -o pid,%cpu,rss,command`.
- `env -i` and an absolute `/bin/ps` skip `PATH`, so they still fail.
- A login shell (`zsh -l`) still fails: macOS `path_helper` puts `/bin` first.
  So a sandboxed Codex session runs its commands without a login shell, and
  your `~/.zprofile` does not load there.
- The stand-in exists only under the sandbox on macOS. With the owner's
  opt-out, and on Linux, `ps` is the system one.

## Only on Linux

Landlock grants by folder, and it fixes the folder set at spawn. So inside
the sandbox nothing can create a new entry at the top of `$HOME`,
`~/.lucidos` or a workspace root. Write into an existing folder instead: the
engine creates `~/.cache`, `~/.config` and `~/.local` before the first
spawn. A hidden entry's name still shows when you list its parent folder,
but its contents stay closed.

Before Linux 6.2 (Landlock ABI 3) the sandbox cannot stop truncation. There
an agent can still empty a protected file, but it cannot read, change or
replace one. Settings → Permissions → Agent sandbox shows a note when this
applies.

## A refused spawn

Where the machine cannot apply the sandbox, the process does not start. The
error names the reason, and the owner gets one notification. The owner can
turn on **Run agents unsandboxed** (`agents_unsandboxed`, Settings →
Permissions). Agents in this workspace then run without the sandbox, and
the app shows a standing warning.

Never ask the user to turn it on to get past a single permission error. It
removes the protection for every agent in this workspace, and only the owner
decides that.
