# 0648: Dynamic bars are on by default on every phone client

- **Status**: Accepted (amends 0553)
- **Date**: 2026-11-10

## Context

ADR 0553 forced dynamic bars on in the installed iOS app and left every other
client on the `mobile_dynamic_bars` default, which was `false`. An Android user
therefore saw pinned bars unless they found the switch in Settings.

## Decision

`mobile_dynamic_bars` defaults to `true`. The installed iOS app still ignores
the preference and always runs dynamic bars.

## Rationale

Dynamic bars are the preferred phone experience, and the iOS app already proves
the shape daily. A default that only one platform gets hides the better layout
from every other phone user.

## Consequences

- Android, Safari tabs and mobile Chrome glide the bars unless the user turns
  the switch off.
- A stored `false` still wins. Only a workspace with no stored value changes.
- The e2e baseline stays pinned. `reset_e2e_database` seeds the preference
  off, so the specs written against pinned bars keep one baseline whatever
  order they run in. Specs that test the glide still turn it on.

## Alternatives considered

- **Force it on everywhere, like the iOS app.** iOS forces it because its
  status bar leaves no working pinned shape. Other clients have one, so the
  switch stays useful there.
