# 0485: The update relay refreshes the gateway's release check before refusing, so a phone offers Update Desktop App without waiting for the hourly poll

- **Status**: Accepted (supersedes [ADR 0338](0338-phone-update-relays-through-the-desktop-app.md)
  in part: its rule that the relay needs a release the gateway already knows)
- **Date**: 2026-10-15

## Context

A phone opened What's New at 12:03, a minute after the v0.49.0 tag. It ran
0.48.1 and saw 0.49.0 as `Newer`, with **How to Update** instead of **Update
Desktop App**. The desktop app was attached and unblocked.

The gateway polls lucidos.dev hourly (ADR 0108), so its `release_check.latest`
was still null. ADR 0338 offered the relay only for a release the gateway
knew, because the gateway refused a request for anything else. What's New
learns of a release from the published changelog, which the gateway's check does
not read. The route fell to `desktop`, and no relay request reached the gateway.

A phone cannot make the gateway look again. ADR 0190 took `check` away from
phones, and What's New never offers it. So the relay worked only in the window
after the next poll, and that window does not hold the moment a user sees a
release.

## Decision

**The gateway refreshes its own release check before it refuses a relay request
for want of a newer release.** On `NoNewerRelease` it forces one poll and asks
again. The gateway stays the authority on what gets installed.

**The frontend offers `relay` when a desktop app is attached and unblocked, and
a newer release is known from any source.** That is the gateway's check, or a
caller that asserts one, as What's New does. Once the gateway knows a release,
its answer outranks the caller's word.

**A phone with a ready desktop app and no known release gets `check`, not
`desktop`.** A release the check finds turns the control into **Update Desktop
App**, so the check now leads somewhere.

**The refusal states the fact:** no release newer than the desktop app is
published yet. "Already up to date" was false whenever the origin lagged the
changelog. When the forced poll itself fails, the refusal says the check failed
and why, because a failed poll proves nothing about what is published.

**Settings, System shows a phone the check button** when its desktop app is
ready, since a phone has no other surface that offers `check`.

## Rationale

The refresh belongs in the gateway because the gateway owns the check and its
coalescing (ADR 0108). The frontend sends intent only, so it needs no new
route to poll first. Every other refusal (no client, a blocker, a run in flight)
returns at once with no outbound request, and the check's own 60 s floor bounds
a refused retry. The refusal order puts a run in flight ahead of the version
test for the same reason.

## Consequences

- A phone can update the desktop app seconds after a release, with no wait for
  the hourly poll.
- A request the origin cannot confirm still queues nothing. The gateway never
  installs a release it has not seen published.
- What's New may offer **Update Desktop App** for a changelog release the origin
  does not list yet. Tapping it shows the gateway's refusal as a toast.
- ADR 0190's suppressed toast and badge on a phone stand.

## Alternatives considered

- **A phone-side "check now" button.** It adds a control and a second request
  path to reach what the relay's own request can do in one step.
- **Have the frontend force the check, then ask.** It moves gateway logic into
  the client, and two requests can race with the hourly poll.
- **Have the gateway read the changelog.** A second source of truth for what is
  published, next to the one ADR 0108 chose.
