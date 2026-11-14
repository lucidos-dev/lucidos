# 0354: The Plugins panel names a failed catalog scan and shows no age

- **Status**: Accepted. Amends [0243](0243-plugin-catalog-is-a-cached-projection.md),
  whose panel showed the catalog's age.
- **Date**: 2026-10-03

## Context

ADR 0243 made the plugin catalog a cached projection. The panel drew a caption
under its filter bar: "Updated just now", "Updating…", "Update failed", or "Not
checked yet". The caption was also a button that started a scan.

The user found it noise. Pull to refresh and the header's Refresh already start
a scan, so the button did nothing the panel lacked. And in the normal case the
caption only said the list was fresh.

## Decision

The panel shows no age and no scanning cue. It draws one notice, and only while
the last scan failed: the reason, and a Try again link. During a retry the
notice stays up and says "Checking again…" in place of the link.

## Rationale

The caption had one job no other surface did: it was the only place a failed
scan showed its reason. A failed scan freezes the list at its last result, so
dropping that would leave stale rows passing as current, with no sign. The
notice keeps that job and drops the rest.

The notice stays up through a retry because the scheduler retries every five
minutes. A notice that hid during each pass would move the list under the
reader twice per pass.

## Consequences

- A healthy panel shows the list and nothing about its age. A list up to five
  minutes old reads as current, which ADR 0243's TTL already accepts.
- `marketplaceScanning` still drives two things: the empty list's "Scanning
  marketplaces…" and the notice's "Checking again…".
- A cold open whose cached catalog holds a failure draws the notice with the
  rows. It pushes the search bar down one line. Only a failed scan pays this,
  and the rows land in the same frame.

## Alternatives considered

- **Remove the line entirely.** Rejected: a failed scan would be silent.
- **Keep a reserved empty line, so nothing ever shifts.** Rejected: it costs a
  blank line on every healthy open to absorb a rare failure state.
- **Hide the notice while a scan runs.** Tried first in this change. It moved
  the list on every scheduler pass while a failure persisted.
