# 0037: A down dependency is reported once by the layer that knows, never inferred N times downstream

- **Status**: Accepted
- **Date**: 2026-08-04

## Context

Docker is where every workspace's Postgres lives in dev (ADR 0014 §6/§7). When
the daemon is down, three separate surfaces meet the failure. On 2026-08-04 none
of them named it, and the user's report is the three symptoms in a row:

> The web-dev.sh runner needs a better way to report that docker is down, now the
> workspaces just didn't start, one time stuck at a black screen with Loading...
> instead of the splash, and previously connected just gives a lot of "Failed
> to ..." error toasts.

**The launcher named the condition and then abandoned the user.**
`check_prereqs` did probe the daemon and printed two lines telling the user to
open Docker Desktop and re-run. It was the only prereq check in the file that
refused to *act*: every missing tool gets `Install <tool>? [y/N]` and an install
command, while the one condition with a one-word remedy (`open -a Docker`) told
the user to go do it themselves and start the launch over. Two quiet lines
scrolled past in the middle of a run, so the outcome read as the workspaces
simply not starting. It also probed with `docker info` while the gateway probed
with `docker version`, and non-Darwin got no daemon check at all.

**A running engine whose database vanished still reported itself healthy.**
`/api/v1/health` was built entirely from process facts (workspace path,
`started_at`, version strings) and never touched the pool. So the engine that was
already up when Docker went down kept answering `"status": "ok"`, the gateway's
health probe passed, and the client flipped to `connected`. `useBootSplashReady`
then held the splash until `isWorkspaceReady` (connected AND `threadsLoaded`),
which could never become true, and `delayedBootStatus` says `'Loading…'` for a
connected workspace: a black screen reading "Loading…" for the full 15s safety
cap.

**Twenty loaders each reported the same outage separately.** `useStartup` fans
out ~20 independent loads. Each correctly surfaced its own failure per
`.claude/rules/frontend.md` "No Hidden Errors", so one dead database became a
column of "Failed to …" toasts, not one of which named the cause.

The gateway's *provisioning* path was already right: since the 2026-08-03
addendum to ADR 0014, a daemon that is not up yet is classified `Transient`,
retried with growing backoff, and rendered on the boot splash as "The Docker
daemon is not running yet. Retrying…". That covers a workspace the gateway is
trying to *start*. It covers neither the launcher nor an engine that is already
running when the database goes away.

## Decision

**The layer that knows a dependency is down says so, once. No surface downstream
infers it from the shape of its own failures.**

Three consequences, one per surface.

### 1. One Docker probe, and the launcher offers the remedy it already knows

`scripts/lib/docker.sh` owns the answer, sourced by `preflight.sh` (launch time)
and `workspace.sh` (provision time). The probe is the **exit status of
`docker version --format {{.Server.Version}}`**, never a matched error message,
deliberately mirroring `docker_daemon_state` in the gateway's `postgres.rs`. That
gateway classifier is what decides retry-vs-latch, so a shell half classifying
differently would tell the user one thing while the gateway did another.
`docker inspect` cannot serve: it exits 1 identically for "no such container" and
"daemon down", which is what made the 2026-08-03 login race surface as a
confusing socket path.

On macOS an unreachable daemon is offered `Start Docker Desktop? [Y/n]` and
waited out with a progress line. Every other outcome (declined, non-interactive,
`open` failed, timed out, no CLI, non-Darwin) prints one visually delimited block
naming the condition, quoting the daemon's own words, and reproducing the
caller's exact command. Non-Darwin gets the same hard check, minus the offer.

### 2. `/api/v1/health` reports `database_reachable`, and still returns 200

A background probe (`engine::db_health`, 5s ticker, 1s per probe) writes an
`AtomicBool` the handler reads. Two boundaries are deliberate:

- **The handler never awaits the database.** An inline probe would put database
  latency on the endpoint the gateway health-checks with a 5s client timeout, so
  an outage could start tripping that deadline too.
- **The status code stays 200.** The status code is about the engine *process*,
  which is answering; the field is about its *dependency*. Failing the endpoint
  would recruit the gateway's respawn machinery against a condition respawning
  cannot fix, and ADR 0014 (2026-06-27) forbids culling an alive engine.

`false` needs positive, repeated evidence: the field starts `true` (the engine
only reaches `serve` after connecting and migrating) and takes two consecutive
failed probes to flip, one success to restore. Per `.claude/rules/rust.md`, an
unanswered probe is not a "no".

### 3. The client renders that one fact and stops rendering the consequences

`checkConnection` mirrors the field into a `databaseReachable` signal. While it
is false, one keyed, non-dismissable error toast names the cause (and, on a dev
install, Docker), and `showToast` suppresses the incidental toasts behind it.

That suppression window is not new: `showToast` already dropped incidental toasts
during an engine restart and a committed packaged update, for exactly this
reason. The three are one concept, so the disjunction became the named
`workspaceUnavailable()` and the opt-out flag was renamed `showDuringRestart` to
**`showWhileUnavailable`** (a name covering three conditions must not mention
one, `.claude/rules/glossary.md`).

The boot splash gains `bootSplashShouldRelease`: ready, OR *known unusable*.
Positive evidence only, so a slow first health response still holds the splash.

## Consequences

- **Suppression is only ever honest with its authoritative toast**, which is why
  that toast is not dismissable and carries no auto-dismiss. A suppression window
  with nothing on screen would be the "No Hidden Errors" violation this is
  otherwise the opposite of: twenty accurate-but-useless reports replaced by one
  accurate and useful one.
- **A database claim never outlives the engine that made it.** It is evidence
  *from* the engine, so a settled disconnect retires it. Otherwise a stuck claim
  would strand an outage toast under a red dot and keep suppressing the
  engine-level toasts that then own the story (an "Engine restart timed out"
  among them, which does not opt in).
- **An older engine is unaffected.** The field is absent there and absence reads
  as reachable; the client never invents an outage from something the engine did
  not send.
- **The gateway is untouched.** It keeps no database access and no new dependency
  (ADR 0014 §1), and its provisioning retry policy is unchanged.
- **Deliberately not done: a picker-wide "Docker is down" banner.** It would be
  the single clearest report when nothing can start, but it needs a new gateway
  control surface plus picker UI, and the per-workspace boot failure already
  names Docker on the splash.
- **Deliberately not done: auto-starting Docker from the engine or the gateway.**
  Only the interactive launcher offers it, because only the launcher has a
  terminal and a human in front of it.

## Alternatives considered

**Fail `/api/v1/health` when the database is unreachable.** Rejected: it conflates
process health with dependency health, and the gateway would respond by
respawning an engine that is working fine, repeatedly, for as long as Docker is
down.

**Probe the database inside the health handler, TTL-cached.** Rejected in favour
of the background ticker. The cache would still make the *first* request after
each TTL pay the probe, on the one endpoint whose latency the gateway's supervisor
reads.

**Flip `connectionStatus` to `disconnected`.** Rejected: an engine answering
`/health` genuinely is reachable, and saying otherwise recruits the reconnect
machinery, the restart toast, and the cold-start picker bounce against the wrong
condition.

**Coalesce error toasts generically (collapse N failures in a window into one).**
Rejected as a symptom fix: it would produce "several requests failed", which is
what the user could already see. The value is in *naming the cause*, which only
the engine can do.
