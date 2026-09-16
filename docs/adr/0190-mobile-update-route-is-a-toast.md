# 0190: A phone's update route is a toast, not the Maintenance page

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

[ADR 0142](0142-no-newer-release-without-a-route.md) made the *update route* a
total function: a surface that can name a release newer than the one running
must also give the route to it. Three values, and no fourth meaning "nothing".

`guide` was the answer for every session that cannot install, and it navigates
to Settings, System, Overview with a Maintenance scroll anchor. That page is
where the whole account lives: the installer command a headless install re-runs,
the Rebuild control a source checkout needs, and the in-app install button.

On a phone it is the wrong page in full. Every control on it is one a phone
cannot work, and the answer it owes is a single sentence that sits among them.
The report was blunt: the link does not make any sense.

The offer toast had the same shape. `Lucidos <v> available` is raised from the
gateway *release check*, which every client polls on mount and on resume. On a
phone it interrupts a reader to report a click somebody has to make somewhere
else. So did the *System attention badge*, whose own rule is that it clears on
the install rather than on being seen.

## Decision

**A mobile client gets one answer about updating, whatever the state: do it on
the desktop app.** It is a fourth update route, `desktop`, and following it
shows a toast and navigates nowhere.

The route is decided first, before every other branch. No offer, no check and no
install can reach a different answer from a phone.

**Nothing nags a device that cannot resolve it.** The offer toast is suppressed
on a mobile client, and the attention badge's update half does not raise there.
The release check itself still runs, so What's New still marks the release
`Available` and the reader can go and look.

## Rationale

The route stays total, which is the whole of ADR 0142. `desktop` is a real
answer, not the absence the ADR bans, and it is the same fact Overview states to
a browser session. What changes is where the sentence is written.

Lucidos ships no mobile client, so `isTauri()` is false on every phone and
`install` was already unreachable there. What the decision takes away is `check`
and `guide`. A check on a phone ends at "up to date" or at this same sentence.
Either answer is already in hand, so the button is a detour. `guide` spends a
page load to say the one line the page has for it.

Suppressing the toast rather than the poll is what keeps the panel honest. The
check is what fills the `Available` chip, and a phone that stops asking would
show a release list that quietly went stale.

The badge follows from its own definition. It clears on the install, so a device
that can never install is a device the mark would never leave.

## Consequences

- A phone shows no update toast and no update dot. It learns about a release by
  opening What's New, which is a place you go rather than an interruption.
- A phone has no Check for Updates button, on What's New or on Overview. The
  gateway still polls hourly for the machine, so nothing goes unnoticed for long.
- Settings, System, Overview keeps every word it had. `guide` still lands there
  from a desktop browser session and from a source checkout.
- `updateControlLabel` gives `guide` and `desktop` the same words. The reader's
  question is the same, so a second spelling would be drift, not precision.
- The mobile gate is `thisDeviceIsMobile()`, which reads the device rather than
  the viewport. A narrow desktop window is still a desktop.
- An iPad in Safari's desktop mode reports "Macintosh" and reads as a desktop.
  It then gets `guide`, which is the old behaviour, so the blind spot costs a
  page instead of correctness.

## Alternatives considered

**Keep `check` on mobile and change only `guide`.** The narrow reading of the
report, and rejected. It leaves a button whose two outcomes are "up to date" and
the desktop sentence, and the second is reachable without pressing anything.
Worse, a forced check that found a release would have had to raise the very
toast this record suppresses, or say nothing at all.

**Suppress the release check on mobile instead of the toast.** Rejected. The
poll is what fills the `Available` chip in What's New, so a phone would show a
list that silently stopped moving. The nag is the toast, not the knowledge.

**Drop the `Available` chip so the button fits on one line.** Considered for the
layout half, and rejected. ADR 0142 records that the chip and the control
coexist, because they state different facts. The chip is what tells a reader the
row is ahead of them, and the wrap costs nothing on a wide pane.

**Let a phone trigger the install remotely.** Out of scope, and not small. The
install runs `tauri-plugin-updater` inside the desktop client: it downloads the
signed bundle, verifies the signature, swaps the `.app`, kicks the launchd
service and re-execs. [ADR 0108](0108-update-check-lives-in-the-gateway.md)
already rejected the nearest thing, a gateway that runs `install.sh` on consent,
on mechanics: `launchctl bootout` tears down the job's whole process group, so a
spawned installer kills itself part way through replacing the runtime. A remote
install needs a detached helper with its own supervision, which is a project
rather than a route.

**Say nothing at all on a phone, and mark no release ahead.** Rejected outright.
That is the defect ADR 0142 exists to close, seen from the other side: knowing
about an update and being told nothing is worse than being told where it happens.
