# 0428: List rows carry no version or update state; the Plugins panel row's button is the one update affordance

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0019](0019-plugins-panel-and-trigger-autoregistration.md)

## Context

ADR 0019 kept a marketplace chip and an Update affordance on a plugin app's
row in the Apps panel. On a phone that row stacked into a column. The chip,
an "Update available" label and the Update button each took a line of their
own, beside an uncapped description. The Plugins panel row carried its own
version badge ("Update from v0.1.0 to v0.2.0") and a Modified chip beside the
name. The user called both lists messy and asked for the versions and the
Modified chip to move to the detail page.

## Decision

- An Apps panel row shows the icon, the name, a two-line description and its
  ⋯ menu. It names no marketplace and offers no update.
- A Plugins panel row shows the icon, the name, a two-line description and one
  button. The version badge and the Modified chip live on the plugin detail
  page. An installed plugin with nothing to open says **Installed** instead.
- Both rows share one grid in `styles/pages.css`.

## Rationale

The Plugins panel row's button already reads **Update** when an update waits,
so a badge beside the name only repeated it with version numbers. The Apps
panel repeated it again, in a second place. It also pulled the plugin catalog
into a view that otherwise reads only the apps list.

## Consequences

- An update is applied from the Plugins panel row, or from the catalog card.
  The update notification already lands there.
- The Apps view no longer loads or refreshes the plugin catalog, and its
  search no longer matches a marketplace name.
- A blocked update's reason shows only on the Plugins panel row.

## Alternatives considered

- **Keep the Update button on the app row, drop only the labels.** Still a
  second place to update from, and the button kept the stacked phone layout.
- **Show the marketplace as a quiet line under the description.** Rendered and
  offered to the user, who picked the row without it.
