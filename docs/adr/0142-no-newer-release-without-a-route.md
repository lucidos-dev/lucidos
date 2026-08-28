# 0142: No surface names a newer release without giving the route to it

- **Status**: Accepted
- **Date**: 2026-08-27

## Context

Settings, System, What's New lists every published release and marks the ones
ahead of the running engine with a `Newer` chip. It offered nothing beside that
chip. The report was blunt: the app knows about 0.31.1, it is running 0.31.0,
and there is no way to get it.

The chip is honest. It says nothing false about the release. What was missing is
the next sentence.

Three sources feed the panel, and they are independent by design:

- The **changelog** comes from the engine, which prefers the published
  `CHANGELOG.md` on the public mirror over its own baked copy.
- The **offer** comes from the gateway's hourly *release check* ([ADR
  0108](0108-update-check-lives-in-the-gateway.md)).
- The **install** is the client's Tauri updater, and only a packaged client
  fronting a bundle can run one.

So a release the changelog knows and no offer names is the ordinary state, not
an edge case. It covers the first hour after every publication. It covers every
release once the user turns the check off. And it covers every release on a
source checkout, which never polls at all.

A second defect sat under it. A gateway that may not poll returns an untouched
snapshot, and the frontend read that as "up to date" and toasted so. The check
never ran, so nothing supported the claim.

## Decision

**A surface that can name a release newer than the one running must also give
the route to it.** No exceptions, and the type system carries the rule: the
shared derivation `updateRoute()` returns one of `install`, `check` or `guide`,
and has no fourth value meaning "nothing".

One derivation, read by every such surface: the offer toast, the What's New
release list, and Settings, System. `updateControlLabel` owns the words and
`followUpdateRoute` owns the click, so a surface cannot invent either.

The `guide` route lands on Settings, System, which therefore has to answer for
every install shape it receives. A source checkout gets a line saying it
downloads nothing, beside the Rebuild & Restart control it already had.

A check that could not run reports a failure, never "up to date".

## Rationale

Knowing about an update the user cannot act on is worse than not mentioning it.
It sends them hunting through Settings for a control that may not be there, and
it reads as a broken app. The truth is duller: their install shape has a
different answer.

The rule is expressible as a total function because there always IS an answer,
for every shape Lucidos ships. A bundle installs in place. A headless install
re-runs `install.sh`. A source checkout rebuilds. A browser session looking at
someone else's install shape is told where the control lives. What varies is the
answer, never whether there is one.

Making it a type rather than a convention is what stops the next surface
repeating the defect. A `null` in the middle of that derivation is how the panel
shipped a chip with nothing beside it.

## Consequences

- Every release row the panel marks as ahead of the reader carries a control.
  Exactly one does: the newest, because the route is a global answer and one per
  row would repeat it down the list.
- Settings, System is a real destination, not just a page a toast points at. It
  owes an answer to whatever arrives there, including a source checkout.
- The chip and the control coexist, except under `install`. There "Update &
  Restart" already says the release is available, so an `Available` chip beside
  it states one fact twice.
- A dev workspace now shows a permanent line in Maintenance saying it runs from
  a checkout. That is one line of noise for the people who read the panel least,
  and it is the only honest landing for the route.
- A forced check on a deployment that may not poll now reports a failure. That
  reads worse than the old "up to date" and is the point: the old answer was
  invented.

## Alternatives considered

**Leave the chip alone and let Settings carry the update.** What shipped, and
what was reported. The panel is where a reader meets the release, so making them
find another page to act on it is the gap, not the fix.

**A control on every row ahead of the reader.** Rejected. The route does not
vary per release, so N rows would carry N copies of one answer. A reader three
releases behind would meet three identical buttons.

**Link out to a download page on lucidos.dev.** Rejected. This repo publishes no
such route, since the site is published from the maintainer's workspace. So it
would ship a link we cannot verify from here, and the in-app updater is a better
answer wherever it exists. `install.sh` is already reachable as the copyable
command a headless install is given.

**Let the panel run `git pull` for a source checkout.** Rejected outright. The
app does not move the user's checkout, and the rebuild control that already
exists is the honest half of that action.

**Show "Check for Updates" everywhere and let it fail where it cannot run.**
Rejected. A button that errors on every press is not a route, and the deployment
gate is knowable before the click.
