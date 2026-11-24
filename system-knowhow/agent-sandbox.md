---
name: Agent sandbox
description: What the agent sandbox hides or keeps read-only on macOS and Linux. Load on "Permission denied" or "Operation not permitted" under ~/.lucidos or .lucidos/, a refused spawn, a failed `sandbox-exec`, "am I sandboxed" or "sandbox probe".
---

# Agent sandbox

Every process an agent steers runs in an OS sandbox (ADR 0645). That covers
your own `run_bash` and `run_python`, scripts, triggers, coding agents,
background tasks, MCP servers and the frontend preview. macOS uses a seatbelt profile, and Linux
uses Landlock. Child processes inherit it, and nothing inside can lift it.

A permission error on one of the paths below is the sandbox working. It is
not a bug to work around. Pick another route, or tell the user what you
cannot do.

Write confinement and the command guard share the OS temporary-directory roots.
Scratch writes use that same definition in both layers.

Under enforced Seatbelt, or Landlock ABI 3 or later, trigger subprocesses write only in
the workspace, OS temp roots, `/dev` and current folder grants.
Existing protections inside those roots still apply. See `system-knowhow/triggers/grouping-triggers.md` § Program grants
for the permission request, and § Only on Linux below for the read-only mounts.

## Is this process sandboxed?

Read `LUCIDOS_AGENT_SANDBOXED` inside the process you are checking:

```sh
printenv LUCIDOS_AGENT_SANDBOXED
```

A value of `1` marks a Lucidos sandbox, including one inherited from an outer engine.
An absent marker alone does not prove that no other sandbox applies.
Disposable packaged tests preserve this supplied marker through their cleared
launch environments so the nested engine recognizes its inherited sandbox.

The engine caches its own availability probe until the process ends.
To test enforcement for the current process, try creating a file directly inside the workspace's
`.lucidos/` folder. Use the actual workspace root, not a repository or worktree root:

```sh
probe="$(mktemp "<workspace>/.lucidos/sandbox-probe.XXXXXXXX")" && rm -- "$probe"
```

Replace `<workspace>` with that workspace's path.
Inside the sandbox, creation must fail with a permission error.
Use an existing `.lucidos/` folder; a missing folder proves nothing.
The probe avoids the writable subpaths listed below.
If it succeeds, it removes only the file it just created.
An inherited sandbox protects the outer engine's workspace, so probe that workspace.

A write into `$HOME` proves nothing about Lucidos sandbox enforcement:

| Process | Creating a new entry directly in `$HOME` |
|---|---|
| macOS seatbelt | Allowed, even inside the sandbox |
| Linux Landlock | Denied inside the sandbox |
| Codex exec protocol | Denied outside its writable roots |

New coding-agent sessions also record `SessionStarted.sandbox` in the event log.
It records the mode resolved for that spawn; older sessions have no field.
See `system-knowhow/thread-events.md` for its values.

The shared policy is defined in `crates/lucidos-core/src/core/agent_sandbox.rs`.

## What it denies, on both platforms

| Path | Inside the sandbox |
|---|---|
| `~/.lucidos/private/` | hidden: no read, no write, no listing of its contents |
| A private file an older release kept directly in `~/.lucidos/` (`local-token`, `secret-keys/`, `snapshots/`, `network.toml`, `run/`, `gateway/` and the rest) | hidden, the same way |
| The socket named by `LUCIDOS_DATABASE_URL` | connectable for gateway-managed bundled databases after disk and effective password checks; its folder stays hidden |
| Paths listed in Settings → Permissions → Private paths | hidden for newly started processes |
| Each gateway's data dir | hidden, except its `workspaces/` folder |
| A workspace's `.lucidos/` | read-only |
| A workspace's `.lucidos/worktrees`, `runtime`, `tmp`, `exhaust` and `staging` | writable |

### Private paths

Add an absolute file or folder path outside workspaces and registered repositories.
Machine-wide paths apply to the workspaces on this Lucidos home.
A workspace can add external paths for its own agents.

New agent processes receive the current lists when they start.
In-process file tools check the current lists before opening a file.
On Linux with Landlock below ABI 3, the sandbox blocks reads and can still allow truncation.
Settings reports the enforcement level for this workspace.

A machine-wide path takes precedence when another workspace already registered a repository there.
On Linux, Landlock can limit new entries in the parent folders of a private path.
Settings refuses private paths that would limit access to the system temporary folders on Linux.

A write-confined process may still write to the OS temp directories. Those are
`TMPDIR` when set, `/tmp`, `/private/tmp`, `/var/tmp` and the macOS per-user
temp directory. The command guard reads the same list for its scratch targets.

The bundled database always requires passwords, including after gateway adoption.
An older adopted engine with a passwordless database URL needs **Switch to new
version** before opening new database connections. Existing connections survive.
Lucidos does not restart that engine automatically.
Older engines missing the gateway's managed-database handoff also need the owner's Switch action before sandboxed Unix-database commands work.

Sandboxed Unix-database processes require a boot-pinned handoff from the gateway's bundled password-only backend.
External Unix databases refuse these processes, even when their current authentication appears safe: their administrators can later change loaded rules.
The owner can explicitly opt out of sandboxing in Settings → Permissions → Agent sandbox.

As defense in depth, Lucidos checks local
authentication rules and tests a fresh admin connection with an incorrect password.
Only an actual password rejection passes. Passwordless access, verification errors
and a five-second timeout refuse the process on both macOS and Linux.
A verification error names the database's own reason and SQLSTATE. For example,
"sorry, too many clients already, SQLSTATE 53300" means the shared database ran
out of connection slots, not that its authentication is unsafe.
TCP development databases and an explicit sandbox opt-out retain their behavior.

The workspace's `data/`, other paths under `$HOME`, the network and the engine's HTTP API remain reachable.

Local e2e runs inside the sandbox. The harness keeps its workspaces, secrets
and Postgres under `~/.lucidos/e2e-root`, which no gateway registers.

## What fails because of it

- **Control-plane `lucidos` acts.** A command that needs the full-authority
  local token cannot read it, such as `lucidos workspaces review`. The CLI
  acts that go through the engine as this thread still work, such as
  `lucidos data write`, `lucidos events emit` and `lucidos workspaces list`.
  The `lucidos workspaces` broker verbs work too: the engine asks the gateway
  for you, under the owner's permission for that action.
  `lucidos spawn-thread --to <name>` reaches another workspace by name.
  `lucidos snapshot restore` and `restore-status` also work: the origin token
  proves the caller, so they need no local token.
- **Reading the host log.** `gateway.log` sits in the hidden gateway dir, so
  `grep` or `tail` on it fails. The command guard also cards each such read as
  a secret read, with no grant to offer. Run `lucidos diagnostic-log query`
  yourself for sanitized engine, gateway and desktop client lines.
  Never give the owner a Terminal command to collect Lucidos logs. See
  `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos diagnostic-log`.
  When the owner needs raw host evidence sent elsewhere, use its
  `prepare-host-transfer` / owner card / `transfer-host-logs` flow. The gateway
  sends only the approved recognized captures to the bound SSH recipient;
  neither approval nor status gives this process raw-log access. Consent
  cannot remove an inherited OS sandbox. Do not retry raw reads.

The command classifier uses the sandbox policy captured for the process that
executes the command, including resolved hidden roots on moved or second
installs. A workspace under an open subtree is a carve-out only when that
process enforces the sandbox; inherited, opted-out and missing snapshots keep
the secret-read card. Traversal remains secret, and the OS checks symlink
targets at open time. No lasting grant covers a secret read. Old command cards
without an engine-stamped offer have only Allow once and Deny.

Secret-read outcomes steer to `lucidos diagnostic-log query` and say not to
retry another spelling. Chat receives that text on allow and deny. Claude
Code can receive it on denial, but its allow payload has no explanation.
Codex's boolean permission bridge carries no explanation on either outcome.

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
- The confinement tests cannot run in place. See § Confinement tests.

Playwright's headless Chromium still runs. Browser e2e is not affected.

### Confinement tests

The confinement tests apply the real sandbox to prove what it denies. These
cover the sandbox crate, engine sandbox wiring, Python and the real-Chrome
browser tests. Chrome applies its own renderer seatbelt, which cannot nest
inside the agent's. The Chrome tests remain no-ops on hosts without Chrome.

Sandbox probes fail when confinement cannot apply, and never skip. The ordinary
suites leave these tests out (`#[ignore]`). Only
`./scripts/confinement-tests.sh` runs them.

Hardening never refuses to start because a seatbelt cannot nest. Diffs reaching
the browser runtime select the confinement suite, just like sandbox wiring.

On Linux the tests first run in place. Only a failed sandbox probe routes
them to a GitHub Linux runner through e2e GitHub mode. A local failure stays
a failure. `--commit SHA` pins the runner leg, as `/harden` does.

On macOS the script tries a seatbelt first and runs here where it applies.
Inside a sandboxed coding-agent session on macOS it asks
the engine through `lucidos confinement-tests run`. The engine then runs the
script outside the sandbox, in the thread's worktree. It runs nothing else
there.

That run needs the owner's `run-confinement-tests` permission. Without one
the engine raises a card: *Allow once*, *Always allow* or *Don't allow*. The run
executes the branch's code with no sandbox, so it can read Lucidos's secrets.
Only the owner can grant it. ADR 0690.

- `/harden` runs them as the `confinement` suite and never raises the card.
  With no grant the suite fails with exit 75: the tests did not run.
- Then run `lucidos confinement-tests run` in the foreground. It holds the
  card open until the owner answers.
- *Allow once* covers that one run. Commit first. The engine records the run
  with the commit it ran at. If it passed and the worktree is still clean at
  that commit, the harden retry round answers `passed` and exits 0. A new
  commit, an uncommitted edit or a later failing run voids that.
- An engine that itself runs inside a sandbox refuses the run, and so does a
  thread with no live coding-agent session. Neither raises a card.
- When GitHub mode is unavailable or produces no verdict, the Linux route
  exits 75 with the reason. It never skips.
- `--here` never leaves the host. The runner uses it, so a runner without
  Landlock fails with 75 rather than recursing.

### `ps` and `top`

macOS ships both setuid, and the sandbox refuses a setuid binary
(`ps: Operation not permitted`). So the engine puts a `ps` first on `PATH`
that runs `lucidos ps`, a stand-in over the kernel's process table. It takes
the options agents type (`ps aux`, `ps -ef`, `ps -o pid,ppid,command -p <pid>`).
It never guesses: where macOS hides a fact from an unprivileged reader, such as
another user's command line, it prints `(comm)`, `0` or `?`. An unsupported
option exits 2. `system-knowhow/lucidos-cli/subcommands-6.md` § `lucidos ps` lists what it
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

A trigger subprocess also runs in its own user and mount namespace, where
the kernel allows unprivileged user namespaces. Everything outside the
trigger's write scope is mounted read-only there, so chmod, chown, touch and
setxattr fail outside the scope, as on macOS. Inside it, files owned by
other users show as owned by `nobody`. Chat and coding agents get no
namespace.

Where the kernel refuses the namespace (a hardened kernel, a default Docker
seccomp profile, an AppArmor user namespace restriction), the trigger runs
under Landlock alone. Contents, creation, deletion and rename outside the
scope stay refused, but metadata changes are not. Settings → Permissions →
Agent sandbox shows a note when this applies.

Neither system closes hard links: a file in a writable folder that shares
its contents with an outside name still accepts writes.

## A refused spawn

Where the machine cannot apply the sandbox, the process does not start. The
error names the reason, and the owner gets one notification. The owner can
turn off **Sandbox agents** (`agents_unsandboxed=true`, Settings →
Permissions). Agents in this workspace then run without the sandbox, and
the app shows a standing warning.

Never ask the user to turn it off to get past a single permission error. It
removes the protection for every agent in this workspace, and only the owner
decides that.

## Diagnostic inbox access

Received diagnostic bundles stay in Postgres, outside git. The agent database
role can read receipt metadata but cannot read the compressed payload, even in
machine-made workspaces. Content reads use the bounded diagnostic bundle query,
with the diagnostic log's time, text, line-count and output-byte limits.

`GET /api/v1/diagnostic-bundles/:id/query` accepts this workspace's own
agent-origin token and refuses app stamps and device/local tokens alone. It
returns bounded timeline records, with no raw-bundle download. Deleted or
expired payloads remain unavailable even if the original delivery is retried.
