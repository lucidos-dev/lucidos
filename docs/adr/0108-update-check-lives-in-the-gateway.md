# 0108: The update check lives in the gateway, polls lucidos.dev, and covers every install type

- **Status**: Accepted
- **Date**: 2026-08-23

## Context

The update check runs in the workspace frontend. `startAppUpdateChecks()` in
`crates/lucidos-app/src/store/actions/app-update.ts` sets an hourly timer, and
`useStartup.ts` starts it when a workspace window mounts. It polls
`https://github.com/lucidos-dev/lucidos/releases/latest/download/latest.json`.
Three defects follow from that placement.

**Most installs have no update path.** The published manifest carries one
platform entry, `darwin-aarch64`. Intel Macs, both Linux targets and every
headless install poll nothing, and `install.sh` never fetches the manifest at
all. An updater whose target key is absent reports no update rather than an
error, so the gap is silent (ADR 0042, finding F10). Those users sit on versions
with known bugs and are never told. This is the lead defect.

**The timer belongs to a mounted webview.** N open workspace windows on one
machine make N polls an hour, for one answer. The gateway is per-machine and
already supervises every workspace, so the right count is one.

**The poll lands on GitHub.** The counter there is one integer, with no
timestamp, no platform and no user agent. We cannot tell how many installs exist
or which platforms they run. This third defect serves us rather than the user,
and this record labels it that way throughout.

## Decision

The gateway owns the check. It polls a lucidos.dev route, announces the result,
and installs nothing. The workspace frontend keeps the install and loses the
timer.

### The route contract

The site is published from the maintainer's workspace, not from this repo. This
section is the contract the published side implements.

```
GET https://lucidos.dev/api/update-check?platform=<macos|linux>&arch=<aarch64|x86_64>&version=<semver>
```

Request:

- `platform` comes from `std::env::consts::OS`, mapped to `macos` or `linux`.
- `arch` comes from `std::env::consts::ARCH`.
- `version` is the baked `LUCIDOS_RELEASE`.
- Exactly three parameters. No cookie, no custom header, no body.
- The client sends no credentials and follows no redirect.

Response:

- Always `200`, with `Content-Type: application/json`.
- Body `{"version": "0.29.0", "notes": "…"}`, or `{"version": null}` when nothing
  is published for that platform and arch.
- `notes` is **optional**, and carries the release's changelog entry as raw
  markdown. Absent, the offer shows no "What's new" link, and the release still
  appears in the panel once it fetches the published changelog.
- Never `404`. An outage and "no build for you" must not look alike.
- Unknown fields are ignored, so the origin can grow the shape later.
- `Cache-Control: public, max-age=300`, and no `Set-Cookie`.

The client compares versions itself, so an origin that always answers with the
newest published version is correct. The `version` parameter lets the origin
answer differently, for a staged rollout or for a version too old to update in
place. It sits in the query, so the cache key includes it. That gives one cache
entry per platform, arch and version, which is a handful of entries.

Two obligations bind the origin:

- **Announce only a fully published release.** ADR 0042 makes this true by
  construction: a release publishes its draft only after all four tarballs
  attach, and the site publisher runs off `LucidosReleased`.
- **Read aggregate counts only, and retain no per-request identity.** The
  request rate per platform is the metric. Nothing about this design needs an IP
  to be stored or joined.

### What the gateway does

- **Hourly, with a staleness gate.** A refresh request polls only when the
  gateway's own answer is older than the interval. N open windows therefore cost
  one outbound request, and opening the app after a gap refreshes at once. The
  timer is the backstop for a gateway nobody is looking at.
- **Fail closed, on two conditions.** `LUCIDOS_PACKAGED=1` must be set, and the
  executable must not resolve inside a source checkout. Both shipped launchers
  set the variable and nothing in dev does. The checkout test is the second half
  because an environment variable can be set by hand.
- **No automatic poll before consent.** The first-run notice must be
  acknowledged first, and the preference must be on. Both gates cover the
  AUTOMATIC check only. The Settings button asks anyway, since that click is
  itself consent for one request. Being able to ask by hand is what makes
  turning the check off safe. The deployment gate is the one nothing bypasses.
- **Announce, never install.** The result rides the existing
  `GET /~/api/v1/control/gateway/status`, in a new nested `release_check` field.
  It carries `last_error`, because a failed poll returns an unchanged answer and
  must never read as "you are up to date".

### What the client does

The install stays where it already works. On macOS
`install_app_update_and_restart` swaps the `.app` bundle and relaunches the
stack, and it runs its own check, so it is self-contained. For an `install.sh`
install the update is a re-run of the installer, and the gateway composes that
command from the live instance. The UI offers it to copy. A browser or PWA
session sees the version and no button, since it can install nothing.

### Consent and the preference

A first-run notice in the workspace picker states what is sent and how often. It
carries "Got it" and "Turn it off", and either answer unblocks the poll. The
persistent switch lives in Settings, System, backed by a machine-global
`~/.lucidos/updates.toml`. There is no environment override, which would
permanently shadow the switch.

### What the request reveals

The payload is platform, arch and version. The request also carries the caller's
IP address, as any HTTP request does, and Cloudflare sees it while terminating
TLS. An hourly poll makes that a presence signal: it shows when an address had
Lucidos running. The phrase "platform, arch and version and nothing else" is a
half-truth on its own. This record and `PRIVACY.md` both state it whole.

## Rationale

**The gateway is the only correct owner.** It is per-machine, it already
supervises every workspace, and it is the one process a headless install runs.
Anything above it multiplies per window, and anything inside the engine
multiplies per workspace.

**Coverage is the argument that carries this, not counting.** Two thirds of the
supported targets have no way to learn a fix exists. That is a security problem
and it justifies a check on its own. The counting is a byproduct, and it is
designed to need no per-request identity: the request rate per platform answers
it, so nothing has to be stored or joined.

**Pointing at our own origin is the weaker half of the decision.** The deciding
argument is release-channel control. We should not need GitHub reachable to tell
a user about a fix, and we want to withdraw a bad release quickly. Counting
quality alone would not have been enough, because the rejected GitHub
alternative below gets most of it for free.

**The check and the install stay separate** because the install genuinely has to
run in the client binary. Nothing about that changes here.

**Nothing installs itself.** The engine already ships this model for plugins:
the marketplace scan notifies and the user decides
(`crates/lucidos-engine/src/scheduler/plugin_updates.rs`). The release check
copies it, including the dedupe marker that stops a repeat scan re-notifying.

**Fail closed, because a dev poll is unrecoverable noise.** A maintainer's own
work must never enter the numbers, and the current design keeps it out for free
by returning early on `tauri::is_dev()`. The gateway has no such property, so
the gate is explicit and refuses whenever it cannot prove the opposite.

## Consequences

- **Lucidos phones home for the first time.** `PRIVACY.md` is rewritten in the
  same change. Its "no telemetry", "does not phone home" and "the headless
  install has no updater" claims all stop being true.
- **We give up a structural guarantee and take a promise instead.** GitHub's
  logs are unreadable to us today, so the user's privacy does not depend on our
  restraint. Now it does. That is a downgrade in kind, and it is the real cost
  of this decision.
- **The poll population changes basis.** The old series counted webviews on
  Apple Silicon Macs. The new one counts gateways on every platform and install
  shape, minus opt-outs, and collapses N windows to one. The direction of the
  jump is not predictable, so the series must be marked at the cutover rather
  than read as continuous.
- **We become a single point of failure for update discovery.** Our origin's
  record is worse than GitHub's. A failed poll is silent, is not retried inside
  the tick, and leaves the last known answer in place.
- **The announcement is not a trust boundary.** A compromised origin can hide a
  version or invent one. It can cause no install, because the install is
  user-initiated and the macOS path verifies the Tauri signature.
- **A non-JSON body is an error, never "up to date".** Cloudflare Pages answers
  an unknown path with landing-page HTML at status 200, which already broke the
  front door once. `install.sh` sniffs for this and so does the gateway.
- **The metric counts gateway instances, not machines.** A packaged app and a
  headless instance on one machine are two processes and two polls.
- **Source installs stay uncovered.** `install.sh --dev` puts the gateway inside
  a checkout, so it never polls and is never notified. That is the fail-closed
  gate working as intended.
- **Skew degrades in both directions** (ADR 0105). An older gateway omits
  `release_check`, and the frontend reads an absent field as "no offer". A newer
  gateway adds a field and removes none, so an older frontend ignores it.
- **One rename.** The gateway's existing `UpdateCheck` struct memoizes whether a
  newer gateway binary sits on disk, which is a different question. It becomes
  `GatewayBinaryCheck`. The `update_available` wire field keeps its name,
  because the workspace picker reads it.

## Alternatives considered

**Per-platform manifests on GitHub Releases.** Publish
`latest-<platform>-<arch>.json` per target, poll the matching one, and difference
GitHub's asset download counters daily for the platform breakdown. This is the
option a privacy reviewer would pick, and it earns an honest summary. It solves
the coverage defect completely, gets most of the platform breakdown, and adds no
new privacy exposure. Moving the check into the gateway fixes the multiplication
defect independently, so that win is kept either way.

Rejected on three counts:

- GitHub's `download_count` is undocumented and CDN-affected.
- It offers no sub-day resolution, and no way to withdraw a release quickly.
- It keeps GitHub a hard dependency for update discovery, including in networks
  that block it.

The margin is thin, and it is recorded as thin.

**Keep the check in the frontend and widen the manifest.** Adding the three
missing platform entries to `latest.json` would fix coverage for Tauri clients.
Rejected: it fixes nothing for a headless install, which runs no webview, and it
leaves N windows polling N times.

**Put the check in the engine.** Rejected for the same reason as the frontend,
one layer up: the engine is per workspace, so three workspaces poll three times.
The plugin marketplace scan has exactly this shape today, against a source the
user chose, which is why it is tolerable there and not here.

**Have the gateway run `install.sh` on consent.** Designed and rejected on
mechanics. On macOS `launchctl bootout` tears down the whole process group of
the job. An installer the gateway spawned therefore kills itself part way
through replacing the runtime. Making it work needs a detached helper outside
the launchd job, with its own supervision and failure reporting. The copyable
command carries the same information at none of that cost.

**Opt-in, off until the user turns it on.** Rejected, though it is the strongest
privacy position. It would also leave most users unaware of releases that fix
bugs they are hitting. That is the defect this record exists to close. The
first-run notice is the compromise, with no poll before acknowledgement. The
user is told before anything leaves the machine, and one click stops it.

**Poll daily instead of hourly.** Considered, and it counts better: a daily poll
fired at startup makes the raw request count read as daily active installs, with
no unique-ing. Rejected on notification latency. The staleness gate recovers
most of the counting benefit anyway, because an install that was off overnight
polls once when its user opens the app.

**Send an anonymous install id so the count is exact.** Rejected outright. It
turns a rate into a tracked population. It is the one field that would make the
request identifying, and no decision needs that precision.

**Invent a new announcement channel.** An SSE event or a dedicated endpoint were
both available. Rejected: `gateway/status` already exists, is already polled by
the surfaces that need this, and is already the machine-global answer. A new
field there degrades cleanly on an old gateway, which a new endpoint would not.

**Let an environment variable override the preference.** Rejected. Environment
beats file in the gateway's other settings. An exported variable would
permanently shadow the Settings switch, with nothing on screen saying why. The
same reasoning already keeps `install.sh --bind` out of the unit env.
