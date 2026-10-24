# 0139: An update check is functional, so it runs on notice rather than a first-run consent click

- **Status**: Accepted
- **Date**: 2026-08-26

Amended by
[ADR 0159](0159-update-check-disclosure-sits-behind-the-explainer.md) on one
point: the Settings row now says what is sent behind its explainer, not at rest
on the page.

## Context

ADR 0108 put the update check in the gateway and gave it three gates. The
deployment must be installed, `~/.lucidos/updates.toml` must have the check
enabled, and a first-run notice in the workspace picker must be acknowledged.
That third gate is what this record reverses.

The notice is a Sparkle-era convention. It is not what comparable tools do:
Claude Code, npm, cargo, `gh` and Homebrew all check for a newer version without
asking first. Each states the behaviour in its documentation and offers a switch.

ADR 0108 named the notice as the compromise that made an opt-out design
defensible. In practice it is not a compromise, because an unanswered notice is
indistinguishable from a refusal. The gate has three real costs.

**Every existing install is bounced to the picker once.** An install that
remembers a workspace never renders the picker, so the notice would never be
seen. The picker had to stand down from auto-opening to show it. Worse, the
stand-down needed the gateway's answer first, so every launch awaited a warm
loopback hop behind the splash before it could reopen anything.

**A new user's first screen is a warning about a network request.** The picker's
job is to open a workspace. Leading with a paragraph about IP addresses frames a
version check as something to be defended against.

**An unanswered notice is a permanent silent opt-out.** Nothing re-asks. An
install whose user closed the window, or opened a workspace by direct URL, sits
at `notice_acknowledged = false` forever and is never told a fix exists. That is
precisely the defect ADR 0108 was written to close.

## Decision

**The first-run notice is removed, and `notice_acknowledged` with it.** A
packaged install checks `lucidos.dev/api/update-check` from first launch, with no
click. `[release_check] enabled` in `~/.lucidos/updates.toml` is the single off
switch, and it defaults true.

**`PRIVACY.md` is the notice, and the Settings switch is the control.** Neither
changes in substance. The Settings row is the only place the product itself says
this. So it states what is sent and how often in the open, rather than only
behind its explainer.

### What is NOT reversed

Everything else in ADR 0108 stands.

- **The deployment gate is unchanged and still fail closed.** `LUCIDOS_PACKAGED=1`
  must be set and the executable must not resolve inside a source checkout. It
  refuses whenever it cannot prove the opposite, and `force` never opens it.
- **The payload is unchanged.** Platform, arch and version, and nothing else.
- **There is still no install id.** ADR 0108 rejected one outright. A rate is not
  a tracked population, and nothing here needs that precision.
- **Announce, never install.** Taking an update is still the user's click.
- **The forced check still works while the automatic one is off.** Being able to
  ask by hand is what makes the switch safe to use.

## Rationale

**An update check is functional, not telemetry.** The distinction is what the
request is for. Telemetry sends data about the user, for us. This request asks a
question on the user's behalf and returns an answer to them: is there a newer
version, and does it fix what you are hitting. The beneficiary of the request is
the person making it.

**Functional processing runs on notice, not on a click.** A click is the right
instrument when a user is giving something up for someone else's benefit. Here
they are not. The honest instruments are a plain statement of what happens and a
switch that works, which is what `PRIVACY.md` and Settings already are.

**Security fixes are the payload.** ADR 0108's lead argument was coverage: two
thirds of supported targets had no way to learn a fix exists. The gate silently
withheld the check from everyone who never answered it. That re-creates the same
gap, for an unbounded fraction of installs, and does it invisibly.

**The counting argument is unchanged and still secondary.** Removing the gate
raises the polling population, which serves us. That is not the reason. It was
not the reason in ADR 0108 either, and this record keeps the labelling honest.

## Consequences

- **Installs that never answered the notice start polling.** The field no longer
  exists and `enabled` defaults true, so they take the default. This is the point
  of the change, and it is also the largest single behaviour shift in it.
- **Installs that answered "Turn it off" stay off.** That answer wrote
  `enabled = false`, which is the same switch as before.
- **A file written before this change still parses.** The raw deserialize refuses
  no unknown field, so a stale `notice_acknowledged` is ignored rather than
  causing a warning and a fall back to the defaults. Pinned by
  `an_install_that_turned_the_check_off_stays_off` and
  `a_file_carrying_the_removed_field_still_parses`.
- **The picker's auto-open loses a round trip.** The awaited `fetchGatewayStatus()`
  existed only to feed the notice. Every existing install stops paying it on
  every launch.
- **The status snapshot loses a key, which breaks skew in one direction.**
  ADR 0108 promised that a newer gateway adds a field and removes none. This
  removes one. An older picker bundle computes `!check.notice_acknowledged`, and
  an absent key is `undefined`, so the notice draws and its buttons cannot clear
  it. Only a page held open across a gateway upgrade reaches that state. A
  reload fixes it, because the gateway serves the picker and a fresh load gets
  the new bundle.
- **The privacy position is weaker by one click and no more.** What the request
  carries, who sees it, and what we do with it are all unchanged. The user is
  still told, and can still stop it.
- **We own the wording now.** With no interstitial, `PRIVACY.md` and the Settings
  row are the only places this is disclosed. Both must stay accurate, which is a
  standing obligation rather than a one-time edit.

## Alternatives considered

**Keep the notice and re-ask until it is answered.** It fixes the permanent
opt-out, which is the worst of the three costs. Rejected because it makes the
other two worse: a recurring interstitial on the first screen is more intrusive
than a one-time one, and it still bounces every existing install to the picker.

**Keep the notice but let the poll run while it is unanswered.** This is theatre.
A notice that gates nothing is a notification. One the user must dismiss is worse
than a sentence in `PRIVACY.md` they can read when they care.

**Show the notice inside the workspace instead of the picker.** It removes the
bounce, since the workspace is the screen an existing install actually opens.
Rejected on the remaining two counts: it is still an interstitial about a network
request, and it is still ignorable into a permanent opt-out.

**Default the preference to false and let Settings turn it on.** This is the
opt-in position ADR 0108 already rejected, arrived at by a different route. It
loses for the same reason: most users would never learn a fix exists.

**Ask on first launch but time out to on.** A notice with a deadline is the worst
of both. It is an interstitial, and its answer does not bind, so it misleads
about what the user's inaction meant.

**Drop the gate but keep reporting `notice_acknowledged: true`.** A constant
`true` makes an older picker's `!check.notice_acknowledged` false, so the stale
tab named above degrades cleanly. Rejected on lifetime. The field would have to
outlive every client that reads it, and nothing measures when that is. A reload
already fixes the one case, and it costs the user a keystroke.
