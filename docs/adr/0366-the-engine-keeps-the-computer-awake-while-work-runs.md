# 0366: The engine holds a named idle-sleep assertion while work is in flight; the gateway and shell caffeinate do not

- **Status**: Accepted
- **Date**: 2026-10-05

## Context

A Mac idle-sleeps when nobody touches it, and on AC power the idle timer can be
one minute. Work Lucidos runs on the user's behalf then freezes.

Two nights showed the cost. A scheduled Google Drive backup slept through
`pg_dump`, resumed in a 45-second dark wake, and failed on the next wake. Its
connection had died while the Mac slept. On another night, nightly coding
agents ran from midnight to five. They froze in every sleep and ran only in the
dark wakes, and the dev backup took 44 minutes instead of 10 to 17.

The packaged app held nothing. The dev scripts ran `caffeinate -im -w $$` for
the engine's whole life. That child also died silently in practice: the power
log showed `ClientDied`, and a day-old `web-dev.sh` had no `caffeinate` child
left.

The user's position is that the computer should keep working while its owner
sleeps. Scheduling a backup at 03:00 already says the user wants it to run.

## Decision

While one or more pieces of work are in flight, the engine holds one IOKit
`PreventUserIdleSystemSleep` assertion. That is the same as `caffeinate -i`.
The engine creates it through FFI with a name such as
`Lucidos (dev): 2 thread turns, 1 background task`, and releases it when the
last work ends.

Work takes an RAII `AwakeHold` (`core/keep_awake.rs`) at five sites:

- a chat-pipeline turn (`ThreadGuard`);
- a coding-agent session (`run_direct_agent`);
- a background task (the watchdog future in `BackgroundBashRegistry::spawn`);
- a Thread Queue entry's work (`ThreadQueue::spawn_execution`);
- a backup (`BackupGuard`).

The dev scripts no longer run `caffeinate`.

## Rationale

**The engine owns it because the engine runs the work.** Its own RAII values
already mark where work starts and ends. An assertion dies with its process, so
a crashed engine cannot leave the Mac awake. Each workspace's engine holds its
own, so `pmset -g assertions` names the busy workspace. Dev and packaged run
the same engine, so both behave alike.

**An RAII hold at each work site, not a subscriber that mirrors status.** A
hold is a field of the value that lives exactly as long as the work. So success,
error, panic, cancel and kill all release it, with no event to miss. A
subscriber counting `running` thread rows would hold the Mac awake forever on a
row that never settled after a crash.

**Idle sleep only.** Lid close, an explicit Sleep, a thermal emergency and a
dying battery all still win, and should. So a backup must survive a sleep
anyway: the resumable upload retries, and a failure says how long the computer
slept.

**A coding-agent session parked on a question still holds.** Its process is
alive and resumes the moment the user answers. The answer often comes from a
phone, and an asleep Mac cannot receive it.

## Consequences

- An idle Mac sleeps as before. A busy one stays awake until the work ends,
  including on battery while the lid is open.
- An idle dev Mac now sleeps too. Before, `caffeinate` kept it awake for the
  engine's life whenever that child survived. Remote access to an idle host
  depends on the host's own energy settings, as on a packaged install.
- The engine carries its first FFI, a small `unsafe` IOKit binding. It is
  compiled only on macOS.
- Other platforms take no assertion. The registry still counts holds and logs
  once that it cannot keep the computer awake.
- Whether the assertion holds a Power Nap dark wake is unverified. A run that
  starts inside one may still sleep, and the retry and the sleep note cover it.

## Alternatives considered

- **The gateway owns it.** The gateway runs no work. Every engine would have to
  report its work over the control API, a second copy of state that drifts.
  A missed report would hold the Mac awake for a workspace that finished.
- **A `caffeinate` child process.** It worked for dev, but the assertion shows
  as `caffeinate`, not as Lucidos and its work. It also died silently in
  practice. IOKit gives a named assertion with no process to lose.
- **A status-mirroring bus subscriber.** It reads as cleaner coupling, but a
  `running` row a crash left behind would leak the assertion.
- **`PreventSystemSleep` (`caffeinate -s`).** It holds the Mac awake on AC even
  against the system's own sleep decisions. The user asked for idle sleep only,
  so their explicit choices always win.
- **Respect sleep and only make work resumable.** That is right for OS
  maintenance like indexing. It is wrong for work the user asked for and
  expects done by morning.
