# 0098: The Access page never mints a pairing code by itself

- **Status**: Accepted
- **Date**: 2026-08-21

## Context

Settings -> Access mints a pairing code and draws it as a QR. A code lives five
minutes and is spent once. A phone often needs longer than that: it has to
install Lucidos before it can pair, then come back to the screen and scan again.

So the section renewed itself. When the code on screen expired, it minted a
replacement, up to six times, while the page was visible
(`docs/plans/2026-08-21-the-installed-app-pairs-itself-on-first-launch.md`,
Phase 5). The reasoning was that renewal keeps every code at five minutes and
single use, where a longer TTL would weaken both.

The user reported it as a bug the day it shipped. They pressed **New code**,
stayed on the page, and watched the code change on its own.

## Decision

Every code on this page comes from a button press. When a code expires the
section says so, drops the cards, and waits. Nothing renews.

## Rationale

The renewal was invisible from where the reader sat. The countdown reached zero
and a different code appeared, seconds after the reader had chosen one. That
reads as the page undoing the press, and a page that undoes a press is not
trustworthy about anything else on it.

It also solved a problem the reader does not have while looking at the screen.
Somebody watching the countdown run out can press the button, and the code they
get is the one they asked for. Renewal only paid off for a screen nobody was
watching, which is exactly the case a bound had to be invented for.

## Consequences

- A phone returning from an install may find the code expired. The reader
  presses the button, which is one tap and now the only way a code appears.
- The section holds no renewal state: no counter, no cap, no in-flight guard
  against its own re-render. `mintPairingCode` has one call site, in the
  callback every button in the section is wired to.
- Nothing in the auth layer moves. The five-minute TTL and single-use redemption
  are untouched, as they were when renewal was added.
- The residual in `docs/known-gaps.md` stands unchanged in substance: a code
  fixed into an installed app's `start_url` was never reachable from this page,
  with or without renewal.

## Alternatives considered

- **Renew, but only past the first expiry the reader watched.** Keeps the
  mechanism and adds a rule about when it is allowed to surprise somebody. The
  reader still cannot predict which code is on screen.
- **Renew silently and say so on the page.** A line explaining that the code
  will change by itself is an admission that the behavior needs explaining. The
  button already says the same thing and costs one tap.
- **Lengthen `PAIRING_CODE_TTL` instead.** Rejected when renewal was added, and
  still rejected: it weakens every code everywhere, including the ones minted by
  `lucidos pair` and the ones carried into an install.
