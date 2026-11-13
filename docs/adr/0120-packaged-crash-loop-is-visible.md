# 0120: A packaged service that crash-loops reports it on the splash, counted by restarts not by a clock

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

The gateway validates the resources it needs at boot: `validate_engine_bin`,
`validate_embedded_pg_dirs`, `resolve_static_dir`. Those checks are fatal on
purpose. A gateway with no staged frontend still answers `/~/api/v1/health`, so
failing fast is the only way the defect is ever named.

In the packaged `.app` nobody was told. `run_service` returns 1, the service
plist carries `KeepAlive` plus `ThrottleInterval 10`, and launchd respawns
forever. The client's start-and-navigate loop records a failure only when
`ensure_service_installed_and_running` itself errors, which it does not here. So
the splash counted up with "Waiting for the background service… (Xm YYs). It may
still be starting up after a restart." at a condition that would never clear.

The reason existed the whole time. It reached
`<app-data>/logs/engine-service.err.log` with the offending path in it, and
nothing read that file back: the only mention of its name in the tree was the
plist writer.

The headless vehicle already gets this right. `install.sh` dies after
`LUCIDOS_HEALTH_TIMEOUT` with the reason and a logs hint, and that is the
behaviour this brings to the `.app`.

## Decision

The client reads the service's error log back, and reports a crash loop once it
has counted `SERVICE_CRASH_LOOP_BOOTS` (3) service starts inside ONE wait. The
report replaces the counting line. It carries the reason the gateway or the
service gave, and the log path.

Both producers stamp `boot failed: <reason>`. `run_service` also writes one boot
marker per launchd start. The client records the log's length when it starts
waiting, and counts only what is written after that point.

The loop keeps retrying. What stops is the counter.

## Rationale

**A restart count separates the two failures; a clock does not.** The condition
to protect is a genuinely slow first boot: Postgres `initdb` plus the embedding
warmup on a cold machine. That boot writes exactly ONE marker for at least
`ENGINE_HEALTH_TIMEOUT` (120s), because that deadline is the only thing that
makes the service exit. A staging-check failure kills the gateway in under a
second, `await_gateway_start` returns `ChildExited` at once, and launchd
respawns after the 10s throttle. So three markers mean at least two service
exits, which a slow but progressing boot cannot produce inside one wait.

**The reason is the FIRST `boot failed:` line of the failing start.** Both
producers write into one file. The gateway dies before the service notices, so
its precise, path-bearing line lands first, and the service's summary of the
same event lands after it. Taking the last match would show the useless half.

**Reading a log is the only channel available.** The splash paints on Tauri's
bundled asset scheme and can reach no HTTP surface until the client navigates
it. Tauri IPC and `startup_status` are all it has. The service is a separate
process under launchd, with its stderr already redirected to that file by the
plist we write. Nothing else connects the two.

**Not giving up is a decision, not an omission.** The loop has never given up,
and a fatal staging defect is repairable from outside: a reinstall, an update, a
moved bundle. The lie was the counter, so the counter is what goes.

## Consequences

- One `SERVICE_ERR_LOG` constant now serves the plist writer and the reader. Two
  literals would let the reader drift onto a file nothing writes, and that drift
  looks exactly like a service that never failed.
- `lucidos-gateway`'s `main` no longer returns `Result`. Rust's `Error: "…"`
  Debug form quoted and escaped the message, so a path-bearing reason reached
  the log unreadable. It prints `[gateway] boot failed: {e}` and exits 1.
- The marker is a cross-crate contract with no linkage behind it, because
  `lucidos-app` must not link `lucidos-gateway` (ADR 0014 §1). A `desktop.rs`
  test `include_str!`s the gateway's `main.rs` and greps it, so rewording that
  line goes red rather than silently costing the splash its reason.
- The splash's status line was one nowrap ellipsized line. A multi-line label
  now switches it into a wrapping report state, decided on the text so no caller
  has to know the state exists.
- The threshold costs about 20s before a crash loop is named: three starts at
  the 10s throttle. That is deliberate, and it is still far inside the 60s at
  which the old line began claiming a restart explained the wait.

## Alternatives considered

**Stop at the first failed start.** Fastest report, and wrong. A service can
exit once for reasons that clear: a port briefly held by a shutting-down
instance, a `bootout` racing a `bootstrap`. The existing loop exists precisely
because one bad cycle is not a verdict.

**Time out on a wall clock instead.** Simple, and it cannot tell the two apart.
A threshold long enough to protect a cold `initdb` leaves a crash loop invisible
behind it. A threshold short enough to catch the crash loop accuses the slow
machine. Neither is knowable in advance, because the slowest legitimate boot is
a property of the user's hardware. The restart count reads the actual
distinction instead of estimating it.

**Parse `launchctl print gui/<uid>/<label>` for the last exit status.** It is
the authoritative source for the respawn count, and it is a text format Apple
does not version. It also says nothing about WHY. The log read would still be
needed, and the client would then depend on two sources instead of one.

**Have the service write a structured failure file instead of a log line.** A
JSON handle beside the log would parse more cleanly. It would also be a second
account of one event, free to disagree with the log a human reads. And the
gateway would need its own writer for the case where it dies before the service
knows. One file, read the way a person reads it, has no such seam.

**Give up and show a terminal error.** Matches the gateway's own
`BootFailure::Terminal`, and the client cannot make that classification: it sees
a dead service, not the difference between an unfixable bundle and a machine
that will be fine on the next launch. Claiming a dead end that a reinstall
disproves is a worse lie than the counter was.
