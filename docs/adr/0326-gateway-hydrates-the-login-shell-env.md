# 0326: The gateway hydrates the login-shell environment, for both shipped installs

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

A process that launchd or systemd starts inherits the service manager's
environment, not the user's. `~/.zprofile` and `~/.zshrc` never ran. So PATH
misses nvm, asdf, mise and volta, and a key exported only in a profile is
absent.

The packaged `.app` fixed this in `lucidos-app`: `run_service` ran the user's
login shell, then applied an allowlist of its variables before it spawned the
gateway. The headless install (`install.sh`, with a launchd plist or a systemd
`--user` unit) runs `lucidos-gateway` directly, so nothing hydrated there. A
headless install could not find `claude`, `codex` or `npx` from a version
manager. The engine's `core::user_path` floor covers only Homebrew,
`/usr/local/bin`, `~/.local/bin` and npm-global.

## Decision

The gateway hydrates itself, first thing in `boot()`, while the process is
still single-threaded. It does so only when `LUCIDOS_PACKAGED=1` is set and
`SHLVL` is absent. `run_service` no longer hydrates, so the `.app` path
hydrates exactly once.

The mechanism is unchanged: `$SHELL -ilc` bounded to 5 seconds, an allowlist of
variables, never overriding a name already set, and PATH merged shell-first.

## Rationale

**The gateway is the one root both installs share.** The `.app` runs launchd,
then `Lucidos --service`, then the gateway. The headless install runs launchd or
systemd, then the gateway. Every engine and coding agent descends from the
gateway. Hydrating there reaches all of them with one implementation.

**The gate names the shipped vehicles.** Both set `LUCIDOS_PACKAGED=1`, and the
dev gateway never does. `SHLVL` still marks a process that a shell started, such
as the headless foreground launch from `install.sh`. That process already has
the user's environment and keeps it.

**`set_var` stays sound.** `boot()` is single-threaded until it builds the tokio
runtime, and the call sits above that line. A gateway unit test pins the order.
The bounded reader keeps its existing contract: it joins its reader thread on
every path that returns output.

**Service start at login is an accepted exposure.** A service can start before a
profile is comfortable to run: a slow version-manager init, a network call, an
agent prompt. The bound already contains it. The shell gets 5 seconds, its
whole process group is killed on timeout, stdin and stderr are `/dev/null`, and
every failure is logged and swallowed. The `.app` has shipped with exactly this
exposure at login since hydration landed.

## Consequences

- A headless install finds version-manager tools and profile-exported keys,
  the same as the `.app`.
- A profile change reaches the stack on the next gateway start, not live. The
  process is multi-threaded after boot, so it cannot re-hydrate safely.
- On Linux, `$SHELL` names the shell. The fallback when it is unusable is
  `/bin/sh`, since zsh is not a Linux default. macOS keeps `/bin/zsh`.
- Some Linux desktop sessions import their whole environment into the systemd
  user manager, `SHLVL` included. There the gateway skips hydration and
  behaves as before this change. That PATH usually came from a shell anyway.
- Values `install.sh` baked into the unit, from `--openai-key` and friends, win
  over the profile under the never-override rule.
- `lucidos-app` no longer carries `shell_env`. The code and its tests moved to
  `crates/lucidos-gateway/src/shell_env.rs`.

## Alternatives considered

**Capture the login-shell environment at install time, in `service.sh`.** It
avoids running a profile at service start. It lost on five counts:

- **It goes stale.** Version managers put the version in the directory, such as
  `~/.nvm/versions/node/v20.11.0/bin`. The next upgrade leaves a dead PATH
  entry until the user reinstalls Lucidos, and a key added later is never seen.
- **It writes secrets to disk silently.** Every allowlisted key in the
  installer's shell would land in the plist or unit, readable through
  `launchctl print` and `systemctl --user show`. Today the unit carries only
  the keys the user handed the installer explicitly.
- **It defeats never-override.** A baked value reads as a deliberate setting and
  outranks every later profile change.
- **It forks the mechanism.** The `.app` would keep runtime hydration, and the
  two installs would drift.
- **The installer's shell is not reliably the login shell.** `curl … | sh`
  inherits whatever ran it: an ssh session, a tmux with a stale environment.

**Keep hydration in `run_service` and add a copy to the gateway.** Two copies
of the same code, and the `.app` path would run the login shell twice per
start. The gateway copy alone covers both.

**Pass the hydrated values to children with `Command::env` instead of
`set_var`.** It would avoid process-env mutation. But the gateway spawns
engines from several sites, and each would need the extra variables threaded
through. Mutating env in the single-threaded prelude reaches every descendant
with one call.
