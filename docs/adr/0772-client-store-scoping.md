# 0772: Store state is scoped to the client, window or workspace that owns it

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

`store.ts` holds the client's shared state and the Settings category list. Each
piece of state has an owner and a lifetime. Several surfaces read the same
facts, so duplicated rules drift apart.

This record restates the design comments of
`crates/lucidos-app/src/store/store.ts`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

**Settings navigation**

- Each category has an authored short title for the header bar. It is never
  derived by shrinking the font.
- Only the header bar reads the short title. The Settings home list, Search
  Everywhere, the history menu and the title tooltip keep the full `label`.
- Home-list groups are headings above rows. They are not a navigation level, so
  every category stays one tap from the Settings home.
- No top-level Settings entry is platform-gated. Gating belongs to a row or
  section inside a category.
- A row is a category, not a single setting. A lone control goes into a section
  of a bigger category.
- A retired subview key maps to the category that absorbed it. Restoring the
  nav stack collapses an unknown key onto `main`. An external
  `NavigationRequested` reports an unknown key.

**Scope and persistence**

- The focused thread id is window-scoped, stored per window, and not
  round-tripped through the server.
- The release whose What's New this client opened is per client, kept in
  localStorage.
- Release notices are per workspace. Answering on one device settles them on
  every device.
- The source view's soft-wrap and the diff side-by-side toggle are persisted.
  They are ways of reading, so the per-file reset leaves them alone.
- `diffWholeFile` is a per-file override and resets when the previewed file
  changes.
- The Plugins marketplace filter is not persisted, because it narrows the
  current view and is not a preference.

**Derived state**

- `activeProgressDialog` is derived from the signal each flow already owns. A
  running restart wins a tie.
- Unread notifications have one source of truth. The bell badge is a projection
  of it, and no separate count is fetched.
- The condition door for a subscription comes from one function. The transcript
  chip and the waiting panel both use it.
- A `PreferencesChanged` carrying a backup key bumps its own signal, a sibling
  of `backupStatusVersion`.

**Previews**

- The file preview modal is not a `panelOverlay` variant.
- The Files panel and the modal each hold their own preview view state, so they
  never share a highlight or scroll target.

## Rationale

- Header space is the narrowest place a title appears. A title that changes
  size between screens makes the bar jitter, and iOS and Material both say to
  write a short title.
- A platform-gated category hides a correctly predicated row from the one
  platform it applies to. One nav shape lets "go to Settings, then X" be true
  for everyone.
- A hand-written modal strands the user at the first site that forgets to clear
  it. Riding an existing running signal means every existing clear closes it.
- A precedence rule makes a restart and update overlap unrepresentable instead
  of racy.
- `panelOverlay` is the content pane's nav-history unit. A glance at a cited
  file is not a destination Back should walk onto.
- Two copies of a rule, such as the condition door, are how two surfaces drift
  apart.
- A backup run finishing and a backup preference changing mean different
  things. Sharing one counter would put a cloud round-trip behind a local
  preference edit.

## Consequences

- A category needs an authored short title wherever the full name does not fit
  the header bar.
- `components/settings/__tests__/settings-nav-structure.test.ts` pins the
  no-gating rule.
- Any new modal should ride an existing running signal and not add a hand-set
  slot. The writable progress-dialog slot is left only for the surface gallery.
- A reading-style toggle must be persisted and kept out of the per-file reset.
- Backup controls re-seed when an agent or another device writes
  `backup_provider`, `backup_schedule` or `backup_retention`.

## Alternatives considered

- A measured font-shrink for long category names in the header bar is rejected,
  because it makes the bar jitter.
- Platform-gating a whole Settings category is rejected.
- Opening the file preview as a `panelOverlay` variant is rejected.
- Widening `backupStatusVersion` to cover preference writes is rejected.
- Re-encoding a repo locator to compare it with the overlay's path string is
  rejected, because the round-trip would have to come back byte-identical.
