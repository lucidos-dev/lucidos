# 0186: The packaged client writes its own diagnostics to a file, because LaunchServices discards them

- **Status**: Accepted
- **Date**: 2026-09-15

## Context

A user's packaged client did not come back after an app update. The update had
worked and a manual start worked. Which stage failed could not be established,
because the client had written nothing anywhere.

LaunchServices hands a launched app a write-only sink for fd 1 and fd 2, and
nothing written there reaches the unified log. That was measured with a
throwaway `.app` on macOS 26, which reported both descriptors as such a sink and
produced no `log show` entry. The two launchd agents are unaffected: the service
and login plists each set `StandardOutPath` and `StandardErrorPath`. The client
has no plist of its own, so it had neither.

Three lines that would have answered this report were among the discarded ones:
the failed service restart, the refused LaunchServices relaunch, and the exit
that could not reach the main thread.

## Decision

`client_log::install` points fd 1 and fd 2 at `<app-data>/logs/client.log` as
the first statement of `run()`, in packaged builds only. The file is appended
to, capped, and rotated to a single `client.log.1` at a start that finds it over
the cap.

## Rationale

A file, in the directory the two agents already log to. The client can then be
asked for one directory when something goes wrong, and `ServiceLogTail` already
reads that directory, so this adds no second mechanism.

`dup2` over the descriptors, rather than a logging facade. Every diagnostic in
the client is already an `eprintln!`, and a facade would have to reach all of
them to be worth anything. Taking the descriptors also captures what our
CHILDREN write, which is the point: the relaunch watcher is a detached
`/bin/sh`, and its inherited stderr is how `open`'s own refusal reaches the log.

Capped, because the one uncapped log we ship reached 579 MB on a developer
machine. One kept generation bounds the pair at twice the cap and still survives
the relaunch, which is the moment the previous launch's last lines matter.

## Consequences

- A packaged client's log is `<app-data>/logs/client.log`, and the line
  `[client] launch starting (pid …, v…)` delimits its launches.
- Development is untouched. It runs from a terminal where stderr already goes
  somewhere, and it shares the packaged app-data dir, which is not its to write.
- The service role is excluded twice: `main` routes `--service` before `run`,
  and `install` refuses that argv anyway. A redirect there would empty the file
  `parse_service_boots` reads to report a crash loop.
- The cap is applied at start only. A client that faults continuously for weeks
  can still pass it. The client writes on a fault, so this is accepted.
- An `eprintln!` in the client is now a durable user-visible artifact. Nothing
  secret may be written to it, the same rule every other log already carries.

## Alternatives considered

- **`os_log` / the unified log.** The natural macOS answer, and it fails on the
  thing we need. Asking a non-technical user to run `log show` with a subsystem
  filter is not a support step, and the retention is the system's call, not ours.
- **A plist for the client.** It would give the client the same
  `StandardErrorPath` the agents get, and it would also mean launchd owning the
  client's lifecycle. ADR 0072 already rejected that: the login agent is
  deliberately one-shot, because quitting the client must not respawn it.
- **A logging crate behind a facade.** More machinery than the problem has. The
  diagnostics are already written; they only lacked somewhere to land. It would
  also miss the child processes, which is half the value here.
- **Truncate at start instead of rotating.** Cheaper, and it throws away the
  previous launch's last lines. Those lines are exactly what a relaunch failure
  leaves behind.
