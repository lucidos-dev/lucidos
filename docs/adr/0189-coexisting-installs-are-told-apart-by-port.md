# 0189: Coexisting installs are told apart by port contention, and the packaged port never moves

- **Status**: Accepted
- **Date**: 2026-09-15

## Context

Three install vehicles can sit on one machine: the macOS `.app`, an `install.sh`
headless install, and a source checkout. Each writes its own binaries, launch
agent, data dir and gateway port. Until now none of them could see the others.

Two of them want port 5252. `install.sh` steps around an occupied one when it
registers a brand-new instance. The packaged app does not, because
`resolve_engine_port` returns its persisted port or the default with no probe.
So an `install.sh` install laid down first keeps 5252, and a DMG installed later
cannot bind it.

That state was completely silent. `wait_for_health` accepts any healthy gateway
on the port, and `await_gateway_start` checks health before it checks its own
child. The service duly reports "gateway healthy; supervising" while its own
gateway dies on every launchd respawn.

"Check for updates" fell back to the Tauri updater, which compares the CLIENT
version. So a current app over an ancient engine answered "Lucidos is up to
date". Neither uninstaller mentioned the other vehicle, so uninstalling and
reinstalling the newest DMG changed nothing.

A tester lost most of a working day to it on 2026-09-15. It ended in a hand-run
Postgres-to-Postgres move of their workspace between the two engines.

## Decision

Two things, and they are separate.

**A deliberate multi-install is told apart from the trap by PORT CONTENTION.**
Two installs on two ports are a supported setup and produce no warning, no toast
and no dialog. Two installs configured for one port produce all three. No marker
file, no environment variable and no setting expresses the difference.

**The packaged gateway's port stays fixed.** It does not probe and does not step
aside, whatever else is on the machine. It reports the collision instead.

## Rationale

**The ports already carry the distinction, so nothing has to be declared.** A
source checkout beside the packaged app sits on 5251 and 5252 and works fine.
The accidental case is exactly the one where two installs want the same number.

A marker file would ask the user to describe a fact the machine already states.
It would be missing on every machine that predates it. And the accidental case
is precisely where it would be absent: somebody who did not know they had two
installs cannot have declared that they meant to.

**The packaged port is a published contract.** Paired-device URLs, the Tauri
capability URL pattern, the mobile connect URL and the *stable gateway port*
glossary entry all key on it. Moving it to dodge a squatter would break every
paired device to fix a case a sentence covers. The asymmetry with `install.sh`
is therefore correct rather than an oversight: the installer's port is a mutable
property of a slug-keyed instance, and the app's is an address others hold.

**Detection belongs in a shared crate, and the CLIENT's copy is load-bearing.**
A trapped user's gateway is ancient by definition and will never run new gateway
code. Their client is current: it is what they just reinstalled. So the scan
lives in `lucidos-installs`.

The gateway serves it on the control plane, for the everyday case and for a
headless install. The client runs it at startup, to reach somebody already
stuck. Two copies of a layout map would drift, and a drifted copy reports an
install that is not there. Same reasoning as `lucidos-tailscale`.

**A version claim is derived from `/health`, not from a new field.** The engine
has reported `release` for a long time, so comparing it against the client's own
version works against a ten-release-old engine. A new field would only ever
describe engines that are not the problem.

## Consequences

- Settings, System, Overview lists every install found, with its version, path,
  port, launch agents and the command that removes it. The list is
  unconditional; only the warning is conditional.
- "Check for updates" gained a fifth verdict, `shadowed`. It is returned before
  the client-updater fallback, so an older engine can no longer be answered with
  a claim about the client.
- The client raises one native dialog per distinct conflict fingerprint, from
  `install_preflight`. An unchanged machine is silent on every later launch.
- Each uninstaller reports the other vehicle and refuses to touch it. The app's
  completion dialog names an `install.sh` instance, and `uninstall.sh` names the
  bundle with the menu item that removes it.
- The layout is now encoded in Rust and in shell. `service_test.sh` reads the
  Rust constants and fails when the two drift.
- The service role logs a pre-held port at boot. It does NOT fail the boot. An
  orphaned gateway of our own reads identically from there, and it is
  serviceable, so refusing would show a failure over a working app.

## Alternatives considered

**A marker file or a setting declaring the multi-install deliberate.** Rejected
above: it asks the user for a fact the ports already state, is absent on every
existing machine, and is missing in exactly the accidental case.

**Let the packaged app step to the next free port.** Rejected. The stable
gateway port is an address paired devices and the Tauri capability pattern hold.
Stepping off it breaks working setups to fix one that is already broken, and it
does that silently.

**Make each uninstaller remove the other install.** Rejected. Removing a second
install nobody named is destructive on the strength of a scan, and the second
install may be the one the user wants. Naming it with its exact removal command
turns a lost day into a five-minute fix without that risk.

**Automatic Postgres-to-Postgres migration between the two installs.** Rejected,
and named out of scope in the prompt that commissioned this. The hand-run
version worked once, under supervision. Detection good enough to name both
databases is what the user actually needs.

**Fail the service boot when the port is already served.** Rejected for now. It
cannot tell a foreign install from an orphaned gateway of our own, and the
second is serviceable. The splash would report a failure over a working app. The
log line plus the client's preflight covers the same ground without that.

**Fold the inventory into `/gateway/status`.** Rejected: the picker polls that
route every two seconds, and this scan walks directories and reads a plist. Same
reasoning that kept the release-check poll off it (ADR 0108).
