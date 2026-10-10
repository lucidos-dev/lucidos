---
paths:
  - ".github/workflows/install-smoke.yml"
  - "scripts/lib/front_door_parity*.sh"
  - "scripts/lib/front_door_gate_test.sh"
  - "scripts/lib/release_rc_front_door*.sh"
---

# The front door: verifying what the origin serves

Scoped to the jobs and scripts that verify the advertised
`curl -fsSL https://lucidos.dev/install.sh | sh`. Split out of
`build-release.md` on 2026-08-06: at 28,511 chars it was a quarter of that file
and loaded on every edit to `install.sh`, `Dockerfile` or a build script, none
of which can change any of it.

**The origin side lives in the dev workspace, not here.** The Pages projects,
the Cloudflare Access gate on the RC origin, DNS, cache purge, and the publish
trigger chain are operations, and the workspace owns them in
`data/knowhow/front-door-verification.md` and
`data/knowhow/cloudflare-lucidos-dev.md`. Read those when the question is what
the origin does. Read this when the question is what the repo asserts about it.
Two consequences of that split are load-bearing here:

- **The Pages deploy does not run in CI.** It runs on the maintainer's machine
  off a workspace trigger chain, so a `release: published` webhook fires
  mid-deploy and would verify the PREVIOUS origin, passing for the wrong reason.
  Production is dispatched by the publisher once its own post-deploy
  verification passed; the RC leg is owned by the `push: rc/**` arm, which
  `release.sh` arms by blocking until the origin serves the candidate before it
  pushes the branch.
- **The repo side can only DETECT a route-set divergence.** Both discovery paths
  live in the site publisher, so the structural fix has to happen there.

#### `front-door`: a parameterised origin, two modes, two host families

The origin is **not** hardcoded. `FD_MODE` + `FRONT_DOOR` are resolved from the
event, and the rung logic is written once per host family:

- **`full`** covers all eight rungs: a real `curl … | sh` on a bare box
  (rungs 1-4), then the advertised **uninstall** paths (rungs 5-8, below).
  Fires on the daily cron and on `workflow_dispatch` (input `origin`, default
  `https://lucidos.dev`). The **post-publish** caller is a dispatch *by the site
  publisher*, not the `release: published` webhook: the Pages deploy does not run
  in CI. It runs on the maintainer's machine off a workspace trigger chain
  (`LucidosReleased` → DMG-link bump → `SitePublishRequested` → publisher →
  `SitePublished`), so the webhook fires mid-deploy and would verify the
  *previous* origin, passing for the wrong reason. The publisher fires the
  dispatch itself once `SitePublished` lands, and passes the release it just
  deployed as the optional `expect_version` input, which is what pins the job's
  two independent fetches of `install.sh` to one release (see below).
- **`payload`**: rung 1 only, then stop green. Auto-runs on every push to
  `rc/**` against the **RC front door** (`https://rc.lucidos.dev`, its own Pages
  project, libs at `/scripts/lib/` on that host), so the soft-404 class is
  caught before anything reaches the real path. Also selectable on a dispatch.
  Rung 1 sniffs the served **uninstaller** too, so an RC is gated on both halves
  of the advertised experience being real shell, not just its installer.

**`front-door-macos` is the same ladder on a Mac, and it is not the `smoke`
job's macOS exclusion sneaking back in.** The landing page shows the
Apple-Silicon DMG directly above the one-liner and routes **Intel** Mac users to
the one-liner outright (the DMG is aarch64-only), so `install.sh` on Darwin is a
first-class advertised path, and it was gated by nothing: `smoke` is
ubuntu-only, `tarball-smoke` and `front-door` are Linux, and `dmg-verify` covers
a different artifact. `smoke` skips macOS because it installs **from source**
(`--dev`), which needs Docker → Colima → nested virtualization a hosted runner
does not expose. The front door tests the **download** path (curl a prebuilt
tarball with its own relocatable Postgres and run it), so none of that applies
and it runs fine on a hosted runner. Same guards, same modes, same origin
validation, same RC payload-only rule; a `fail-fast: false` matrix over
`macos-latest` (aarch64) and `macos-15-intel` (x86_64, the **last** Intel image,
retiring with macos-15 in Fall 2027), each asserting its own `uname -m` so a
re-pointed label cannot leave two legs testing one triple. The one substantive
difference is the **launch shape**: the Linux job runs in a container with no
service manager, so `install.sh` degrades to a foreground launch and an exited
installer is always a failure; on macOS `launchctl` exists, so it registers a
launchd job and **exits 0** with the gateway detached. The macOS job therefore
must NOT fast-fail on the installer exiting: it polls health to the deadline
and reports the recorded exit status alongside a failure, and **warns** when the
gateway is healthy but the installer exited non-zero (a one-liner the user
watched fail, which `KeepAlive` then papered over; a warning rather than a
failure because the likeliest cause is `install.sh`'s own 120 s health wait
expiring on a cold runner). Its teardown is now **cleanup only**: it kills the
installer pid and `launchctl bootout`s any leftover agent, so no LaunchAgent
outlives a run whose rungs failed early. It used to be the file's only uninstall,
piping `--uninstall --all --purge` behind a `|| true` (so it could not red the
job) with a canary branch that printed "does not serve uninstall.sh yet … Not a
regression" on the soft-404. Both are gone: a step that cannot fail is not a
test, and that branch would have swallowed the exact regression rungs 5-8 exist
to catch.

**Uninstall is a real gate as of 2026-07-30 (rungs 5-8, full mode, both jobs).**
Four asserting rungs after rung 4, alternating the two advertised entry points so
each is exercised for both a listing and a mutation:

| rung | command | asserts |
|---|---|---|
| 5 | `install.sh \| sh -s -- --list` | the DELEGATION path: `dispatch_uninstall` fetched `<origin>/uninstall.sh` (its printed URL is checked), the uninstaller's own banner is in the output, no HTML or shell-parse cascade, exit 0, and the instance is listed. macOS also asserts `--list` reports it as `(launchd loaded)`. |
| 6 | `uninstall.sh \| sh -s -- --list` | the DIRECT piped path, same floor. On Linux this is also what exercises `uninstall.sh`'s dash re-exec (`/bin/sh` is bash on macOS, so only the Linux leg reaches that branch). |
| 7 | `install.sh \| sh -s -- --uninstall --all` | the **data-safe** promise from `uninstall.sh --help`: the data dir survives. macOS additionally asserts the launchd agent is booted out and its plist deleted. |
| 8 | `uninstall.sh \| sh -s -- --all --purge` | the instance data dir **and** the shared runtime are actually gone. The runtime is the large half and is deleted only by this branch, so nothing else in the file covers it. |

Two properties of the sequence are load-bearing. The **order saves a reinstall**:
a data-safe rung 7 keeps the data dir *and* its port marker, so rung 8 still has
a target. And the rungs **accumulate** rather than exiting at the first failure,
so one run reports all four verdicts instead of hiding 6-8 behind a rung-5 red.

**The macOS legs WAIT for the instance to be discoverable before rung 5, because
health is not the installer's finish line there (2026-08-05).** `register_service`
bootstraps the LaunchAgent first and calls `record_instance_port` second, which
is the deliberate ordering described under "Discovery is the
`<prefix>/<slug>/port` marker" in `.claude/rules/build-release.md`, and launchd
starts the gateway the moment
it accepts the bootstrap while `register_launchd` is still confirming the load.
So the gateway can be answering `/health`, which is all rung 3 waits for, while
the marker is not yet written, and `service_list_instance_names` keys on that
file and nothing else: `--list` reports no instance and rungs 5, 6, 7 and 8 all
fail off it. Two runs of `macos-15-intel` 22 minutes apart proved it a race
rather than a platform bug, identical through rung 4 and differing only in
whether the installer had already exited. The step therefore opens with a
bounded poll for the marker (`FD_MARKER_WAIT_SECS` 60, `FD_MARKER_POLL_SECS` 2)
that also stops early once `INSTALL_PID` is gone, and its two expiries are
different verdicts: a marker missing after the installer exited is a
`record_instance_port` regression and SHOULD red, while a deadline reached with
the installer still running is a harness budget to raise. It is **not** the
Linux fast-fail creeping back in (that one aborts *because* the installer
exited; this one only stops waiting), and the gate test's negative assertion
still holds, being scoped to the health step.

**The macOS annotations were the other half of that fix, and the reason is worth
keeping: a diagnostic that offers a closed set of causes is wrong in exactly the
case it omits.** The macOS `marker_diagnosis` named two, an installer defect in
`record_instance_port` or an origin regression, and the real cause on 2026-08-04
was the third, the harness arriving mid-registration. It sent a reader hunting a
bug that was not there. It now reports rather than attributes: since the wait
guarantees the marker existed when the step began, a missing one means it
VANISHED mid-step (suspect the rungs, since a data-safe rung 7 promises to keep
it), and the never-written verdict is explicitly delegated to the wait's own two
errors, which are the ones that can actually fire for it. Rung 5's
`(launchd loaded)` check was corrected the same way: it used to convict
`instance_status` of misreading launchd, when an agent that left the domain
between rung 3 and the probe reads identically. It now asks `launchctl print`
directly at the moment the rung disagrees and lets the two answers name their
own cause (still bootstrapped means `instance_status` is wrong; not bootstrapped
means it told the truth and the agent died in between). Pointing at the
**teardown** output instead would not work, and the reason generalises: rung 7
boots the agent out itself, so "nothing left behind" is the expected outcome on
every run and discriminates nothing. A diagnostic has to be read at the moment
its subject is still observable. **The Linux copy of `marker_diagnosis` was
deliberately left asserting an installer defect, because there it is true**: with
no service manager the install always takes the foreground shape, where
`record_instance_port` runs before the `exec`, so a marker missing at rung 5
really does mean the installer failed to write it.

What the **Linux** leg cannot cover, and does not pretend to: the container has
no launchd and no systemd, so `install.sh` registered nothing for a removal to
remove, and `remove_instance` stops the embedded runtime only when it actually
unregistered a service. The foreground gateway therefore survives the uninstall
by design. The macOS legs carry the service half. That same absence is why the
Linux leg needs no twin of the marker wait: with no service manager the install
degrades to `launch_runtime`, which writes the marker BEFORE an `exec` that
never returns, so the ordering is inverted and the marker is on disk before
anything can answer health.

**The `/uninstall.sh` route now exists, and these rungs went green with it
(verified 2026-07-31, post-v0.18.3).** They were fail-closed red for two days
while production soft-404'd that route, which was correct behaviour rather than
a bug to paper over: no change in this repo could have turned them green,
because the publisher owns the route list. It now serves a real 23 KB
`#!/usr/bin/env bash` uninstaller and the publisher lists the route with a
fail-closed guard. **The consequence is that the expected path INVERTED**: the
macOS teardown should now take the delegation path, and a run reporting the
`launchctl bootout` fallback is a regression in the published route rather than
the normal case. The named-outcome branch stays as the detector for exactly
that.

**Payload mode must never run the install, and this is not a gap to close.** An
RC `install.sh` bakes `LUCIDOS_DEFAULT_VERSION=<rc version>` and resolves its
tarball to `…/releases/download/v<ver>/…`, but during an RC **that tag does not
exist**: Phase A publishes only an `rc-<ver>` draft release carrying the DMG +
updater `.sig`, and headless tarballs live solely on real `v*` releases. Wiring
the install in would 404 at the download step on every single run and the gate
would be permanently red. Nothing is lost: the bug class the RC gate exists to
catch is the soft-404, and rung 1 catches it entirely by fetching and sniffing
payloads. Rung 2 cannot substitute, because it asserts over the *log of a real install*,
so it needs exactly the tarball that does not exist.

Four properties keep a payload-mode green honest, and all four are load-bearing:

- the lib base derived from the served installer must equal **exactly**
  `$FRONT_DOOR/scripts/lib` (a prefix match let the apex vacuously satisfy an
  `/rc` base), and a mismatch is **fatal** in payload mode (where rung 1 is the
  only rung) while staying a warning in full mode (where rungs 2-8 still drive
  the origin);
- the served **uninstaller** must pin its own two URLs at the same origin, with
  the same fatal/warning asymmetry, through the shared `pin_mismatch` helper:
  `LUCIDOS_UNINSTALL_SELF_URL` (where a piped run re-fetches itself, since
  `curl … | sh` leaves no readable `$0` to re-exec) and the lib base derived
  from its own baked `LUCIDOS_INSTALL_URL` (where its `service.sh` comes from).
  Both matter because `install.sh` does **not** export its copy of
  `LUCIDOS_INSTALL_URL` before `exec bash -c "$payload"`, so even the *delegated*
  uninstaller re-resolves these defaults: an unpinned copy runs GitHub main's
  script, and the rung would pass having touched nothing at this origin;
- on an `rc/**` push the served installer's baked `LUCIDOS_DEFAULT_VERSION` must
  equal the version in `rc/<version>`, so the previous RC's copy sitting at the
  same URL cannot pass the gate. **A mismatch is re-read before it is believed
  (2026-07-31).** `release.sh` arms the gate by blocking until the origin serves
  the candidate (`scripts/lib/release_rc_front_door.sh`), but that wait polls
  from the maintainer's Mac and so sees exactly ONE Cloudflare POP; a runner
  resolves to another, whose edge cache can still hold the previous release's
  copy. v0.18.3 and v0.18.5 both reddened a front-door leg for that reason alone
  and both passed on a bare `gh run rerun --failed` (on v0.18.5 only
  `macos-latest` reddened while `macos-15-intel` and `ubuntu` read the correct
  version off the same origin, which is a per-POP cache and not an origin
  regression). So rung 1 now polls the mismatch case, and **only** that case on
  the push arm, to a bounded `FD_RC_VERSION_WAIT_SECS` (240 s) every
  `FD_RC_VERSION_POLL_SECS` (30 s), each re-read defeating the edge cache with a
  per-attempt `?cb=` nonce (Cloudflare's cache key includes the query string)
  plus no-cache headers, re-running `assert_shell_file` on the fresh copy, and
  exporting `FD_SERVED_VERSION` only after the loop so a retried value cannot be
  shadowed by the stale first read. Converging late emits a `::warning::` naming
  the elapsed seconds; expiry is still a hard failure, worded so the exhausted
  budget rules propagation lag out;
- the lib-name scrape, the `LUCIDOS_INSTALL_URL` parse, the version parse and
  both uninstaller-pin parses all **fail closed**. A parser that finds nothing
  must never report green.

**The arm now needs two vantages, and two vantages are still not two POPs.**
`rc_front_door_confirms_version` reads the plain URL, which is what a stranger
on this POP gets. It also reads a nonced no-cache URL, which is what the origin
holds. It arms only when both serve the candidate, which separates "the
publisher has not deployed" from "it deployed and propagation is running". It
records the answering POP from `cf-ray`, so a red leg can be compared against
what the Mac saw.

A genuinely different POP is out of reach. Anycast offers no route to one, and
a third-party fetch service is not a dependency a release gate takes. So the
bounded re-read above stays the load-bearing half.

**One propagation fault reds up to three legs**, because each resolves its own
POP. Both the expiry error and the convergence warning therefore state that the
verdict is per-POP and that siblings report independently. Legs quoting the
same last-read value are one fault, not three. Reducing the legs was considered
and rejected: `front-door-macos` on the `rc/**` push is exactly what caught
v0.18.5.

The **full** mode is still deliberately **not** on the `rc/**` push: it tests
production, not the RC tree, so a live-site outage must never be able to block
cutting a release. Payload mode is the inverse: it gates the RC's *own* copy.

**A dispatch naming the RC origin is REFUSED, not downgraded (2026-07-31).** The
validate step classifies the origin (host begins `rc.`, or the legacy `.../rc`
path form) and exits non-zero when it is an RC origin on any event other than
the `push`, **before any fetch**. The refusal names the `push: rc/**` arm as the
RC leg's owner and points at the caller, the workspace trigger
`verify-front-door-after-publish`. This was decided **against** the obvious
alternative of demoting the run to payload mode and carrying on, and the reason
is worth keeping: the only known cause of a dispatch at the RC origin was that
trigger's `/rc` suffix filter, written for the old path-based route and silently
dead once the RC front door moved to its own host on 2026-07-30. That filter has
been fixed, so an RC origin arriving on a dispatch now means something regressed
again, and a downgrade would absorb the evidence of exactly the regression this
job exists to surface. There is no `FD_MODE` rewriting anywhere in either job,
and the drift test asserts that. A fail-closed companion refuses an RC origin
whose `FD_MODE` is not `payload`, so an edit to the job-level expression reds in
the validate step rather than driving a full install at an origin with no
`v<version>` tag.

**The full rungs wait for the release assets, and a download failure is named as
one.** On the v0.18.0 release all three legs failed identically: the post-publish
dispatch fired at 05:58, install.sh printed its own `Download failed:` for the
headless tarball, and each leg then burned the entire 900 s
`GW_HEALTH_TIMEOUT_SECS` before reporting *"the gateway never reported healthy"*.
Nothing was broken. `release-tarballs.yml` had not finished attaching the assets
until 06:21. Three faults in one shape, and all three are now covered:

- **An asset preflight** runs between rung 1 and the launch step, full mode only.
  It polls the tarball **and** its `.sha256` (install.sh's checksum step is
  mandatory and fails closed, so a missing sidecar is just as fatal) with a
  one-byte range request, `FD_ASSET_POLL_SECS: '30'`, bounded by
  `FD_ASSET_WAIT_SECS: '300'`. **That budget stopped being a wait for a build on
  2026-08-04**: a release is now created as a DRAFT and published only once
  `release-tarballs.yml` has attached all four tarballs, so this absorbs CDN
  propagation and nothing else. It was 1800 s, then 3600 s for one day (after
  v0.21.0's Intel tarball landed 3m33s past the old ceiling), both sized for an
  attach window that no longer exists. `front_door_gate_test.sh` caps it at
  900 s so re-inflating it reds a test rather than a release. The URL is what install.sh will really fetch: the
  version comes from rung 1's parse of the **served** installer's baked
  `LUCIDOS_DEFAULT_VERSION` (what a piped run resolves, since no checkout means
  no adjacent `RELEASE`), the base URL is **derived** from the served
  `install_common.sh`, the stem is drift-guarded against the served
  `headless_tarball.sh`, and the triple uses install.sh's own `uname` map (the
  matrix `TRIPLE` on macOS). Every parse fails closed. Rung 1 therefore writes
  its payloads to a stable `$RUNNER_TEMP/front-door-payloads` instead of a
  `mktemp -d`, so the preflight consumes what it already fetched rather than
  asking the origin a second time.
- **Expiry fails fast**, naming both URLs and `release-tarballs`, and never
  reaches the gateway poll. Its wording is now the opposite advice: a PUBLISHED
  release missing an asset is a fault rather than a race, so the message says
  the release is complete by construction and names the two real causes (it was
  published with `--allow-missing-tarballs`, or the asset was removed). The
  drift test pins that, because "wait longer" was correct under the old
  ordering and is misdirection under this one.
- **`assert_no_download_failure`** joins `assert_no_html_payload` inside both
  health polls, matching install.sh's own asset-fetch aborts (`Download failed:`
  and the checksum-sidecar one, not the checksum *mismatch*, which is a
  different verdict). It matters most on macOS, which deliberately has no
  installer-exited fast-fail. Since 2026-08-03 it also reads the version out of
  the URL the installer died on: when that differs from the one rung 1 verified,
  the verdict is **front-door version drift** rather than a download failure
  (next block). A URL it cannot parse a version from falls through to the plain
  wording, so the branch renames a failure and never invents one.
- **`timeout-minutes` is 75**, not 30: the ceiling has to exceed the budgets it
  contains, and 300 + 900 + 900 s of slack is 2100. The convergence budget in
  the next block can stack on top of the preflight's, which puts the worst case
  at 600 + 300 + 900 + 900 = 2700 s, well inside the 4500 s ceiling. It went
  30 -> 75 when the preflight arrived, 75 -> 105 while the preflight's own
  budget was an hour, and back to 75 with that hour. `front_door_gate_test.sh` does that arithmetic over all three
  budgets rather than restating it.

**The job's TWO fetches of `install.sh` are pinned to ONE release
(2026-08-03).** The preflight above certifies the assets of whatever version
rung 1 read, and that guarantee was void in the window it exists for. Rung 1
reads the served installer ONCE; the "Run the advertised command" step
re-fetches the same URL independently, seconds or minutes later; nothing tied
the two reads to the same release. Right after a publish they legitimately
disagree, because the dispatch fires within seconds of the deploy and Cloudflare
POPs are mid propagation. On v0.20.1 the ubuntu leg read 0.20.1, waited out the
attach window and went green, while BOTH macOS legs read 0.20.0, passed the
preflight on attempt 1 against the PREVIOUS release's assets, and then died when
the install step resolved 0.20.1 from a POP that had flipped. The shipped tree
was fine every time: the preflight had verified a release nobody downloaded.

The caller now names the release it dispatched the run to verify, through the
optional **`expect_version`** input (a bare release number, never a `v<ver>`
tag; shape-checked in the validate step so a typo fails there rather than after
the whole budget). Rung 1 converges on it before exporting `FD_SERVED_VERSION`,
bounded by `FD_EXPECT_VERSION_WAIT_SECS` (600 s) every
`FD_EXPECT_VERSION_POLL_SECS` (30 s), re-running `assert_shell_file` on each
re-read so a copy landing on the soft-404 page is reported as HTML instead of as
one more mismatch, warning when convergence needed one, and failing closed with
a message that separates "the publisher has not deployed this release" from
"this POP is still lagging". The workspace trigger
`verify-front-door-after-publish` passes the field, guarded on the
`SitePublished` payload actually carrying a `release_version` (its `"?"`
fallback would otherwise spend the budget waiting for a release nobody
published). An EMPTY input is a first-class case that keeps the previous
behaviour exactly, one read and no polling, which is what stops the daily cron
(long after the assets have settled) gaining a new way to red.

**No nonce, and its own budget: both deliberate, and the first is the one a
future reader will "fix".** The budget is separate from `FD_RC_VERSION_*`
because the two loops watch different origins for different phenomena, so tuning
one must never retune the other. The nonce is the load-bearing difference. The
rc loop above is RIGHT to cache-bust: payload mode stops after rung 1, so its
nonced fetch is the last word and forcing a MISS is the fastest route to the
truth. Here it would be actively wrong, because a `?cb=` query string is a
DIFFERENT Cloudflare cache key: converging on the nonced URL would prove nothing
about the plain one, and the plain one is the only URL the install step ever
requests. What this loop has to assert is the stranger-visible truth, that this
POP serves the expected release at `<origin>/install.sh` to an unadorned `curl`,
so it polls exactly that. Because the contrast reads like an oversight, the
drift test pins both halves: the nonce and no-cache headers present in the rc
loop, absent in this one.

**Both jobs are pinned by `scripts/lib/front_door_gate_test.sh`**, an offline
drift test whose subject is the workflow file itself. It cannot be a unit test:
these jobs only ever execute in the public mirror, so nothing local can run them
before a release does. It asserts every invariant above that is checkable from
the file, **once per job**, since the two are duplicated deliberately and silent
divergence is the standing hazard: the preflight's position relative to the
launch step, the budget band, the fail-closed branches, the in-poll download
check, the timeout arithmetic, the RC refusal preceding the first `curl`, the
absence of any downgrade, the Access-vs-soft-404 distinction, the untouched
guards, and the rc version re-read (its bounded budget and interval band, the
`cb=` nonce and no-cache header on the retry fetch, the retry staying scoped to
the push/mismatch path while the empty-parse branch still exits immediately, the
expiry still exiting rather than warning, and `FD_SERVED_VERSION` being exported
after the loop). It covers the `expect_version` convergence the same way, and
one of those assertions is a NEGATIVE: its own bounded budget and interval band,
the loop sitting behind a non-empty guard, the re-read fetching the plain
`$FRONT_DOOR/install.sh` **with no `cb=` nonce and no cache headers** (the whole
point of the loop, and the opposite of the rule one paragraph up), the payload
re-validated on every re-read, the expiry naming both causes and still exiting,
the export landing after this loop too, the malformed-input refusal preceding
the first fetch, and the drift rename in `assert_no_download_failure` not having
eaten the plain download verdict.

Both export assertions compare against where the loop's block **ends**, not
against its `while`, and that is not pedantry: compared against the `while`, an
export moved INSIDE the body passes while pinning whatever intermediate read the
loop was holding, which is the exact shadowing both loops exist to prevent. The
`guarded_block` helper is what makes that checkable, and it is the same scoping
that keeps the "no nonce here" assertion from being satisfied by the rc loop's
nonce sitting a few lines above. It strips comment lines first, so a job's prose about a rule (the macOS
job documents the ABSENCE of a `kill -0` fast-fail in words) can neither satisfy
nor violate one, and it re-checks the preflight's URL construction against the
tree's real `install_common.sh` + `headless_tarball.sh` and its download-failure
pattern against the real `install.sh`.

**A Cloudflare Access login page is not a soft 404.** The RC origin sits behind
Access; an unauthorized fetch 302s to a login page that `curl -L` follows and
that arrives at **200 as HTML**, which the first-byte-is-`<` test reads as the
Pages SPA fallback. That is how a dispatch at `rc.lucidos.dev` was reported as an
origin regression when the real cause was the service token. The installer fetch
records `%{url_effective}`, and `assert_shell_file` consults it before falling
through to the soft-404 wording. The unset-token case is separately refused up
front, so reaching the auth message means a token was sent and rejected.

The `origin` dispatch input is treated as hostile: the job pipes what the origin
serves into a shell, so a validation step accepts only `https://host[:port][/path]`
over a strict character allowlist, normalises trailing slashes, and exports the
result under a *different* name (`FRONT_DOOR_INPUT` → `FRONT_DOOR`) so a skipped
validation leaves consumers with an unset variable under `set -u` rather than a
usable one. The origin reaches the `sh -c` launch as a positional argument, never
string-interpolated. The job keeps `permissions: {}`.

#### `front-door-parity` is the only job that sees BOTH origins

Every job above, the two front-door ones included, resolves **one** `FRONT_DOOR`
per run. So none of them can notice that production and the release candidate
serve **different route sets**, which is a failure with its own cause: the site
publisher decides the route set twice, in a production route-discovery path and
a separate release-candidate one, and the two can drift. On 2026-07-30
`/uninstall.sh` became a publish route and only the production path learned
about it; nothing noticed until the `rc/0.18.0` push the next morning red all
three payload legs at once.

`front-door-parity` closes that. It runs `scripts/lib/front_door_parity.sh`,
which derives each origin's route set from **that origin's own** served
`install.sh` and `uninstall.sh` (never from a list, never from the checkout),
probes the union at both, and reports per route which origin serves shell.

- **Daily cron and dispatch, never the `rc/**` push.** The divergence exists
  from the moment the publisher's two paths differ, so the daily cron names it
  within a day and independently of any release. The rc push is the *latest*
  point (it is where the 2026-07-30 omission surfaced), the RC leg already reds
  there, and a candidate that legitimately ADDS a route would red it falsely.
- **Severity is asymmetric on purpose.** Production-serves / candidate-missing
  is **fatal** (always wrong, actionable now). Candidate-serves /
  production-missing is a **warning**: an in-flight candidate leads production
  until publish, and making it fatal would red the cron through every release
  window. It stays covered, because at publish production's own installer starts
  declaring the route and `front-door` rung 1 reds fatally. Missing at **both**
  is a warning, since that is `front-door`'s verdict to give in the same run.
- **It checks out the repo and `front-door` must not, which is not a
  contradiction.** That rule keeps the *subject* honest; here the tree is the
  instrument. The checkout buys ShellCheck coverage through `make lint`, a
  hermetic offline test, and one definition of the derivation instead of a third
  copy of bash in a YAML string.
- Offline-tested by `scripts/lib/front_door_parity_test.sh` (two `file://`
  origins). It pins the three severities, every fail-closed parse, the lib-base
  **equality**, that the scrape still matches the **real** `install.sh` +
  `uninstall.sh` (a partial-miss guard the `fewer than 2 libs` floor cannot
  give), and that `FDP_RC_URL_DEFAULT` equals `rc_front_door_url`.

**The repo side can only DETECT this.** The two discovery paths live in the site
publisher, on the maintainer's machine, so the structural fix (one discovery
function parameterised by destination) has to happen there.

