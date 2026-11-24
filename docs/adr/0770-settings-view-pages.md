# 0770: Each Settings page answers one user question

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0098](0098-pairing-code-no-auto-renew.md)

## Context

`SettingsView.tsx` renders the Settings pages. Several of them once split one
user question across separate lists or categories, or treated a missing
optional input as an error.

This record keeps the structural decisions that the comments in that file
carried.

This record restates the design comments of
`crates/lucidos-app/src/components/settings/SettingsView.tsx`, written before
the comment strip removes them. Nobody checked it line by line against the
code.

## Decision

- **Registry.** A failed or absent registry is not an error state on this page.
  It degrades to an empty list, not a `LoadableError`. The result is no quick
  buttons and no autofill. The typed-name path still connects.
- **Devices.** There is one list of every device, with two actions on each.
  `buildDeviceRows` joins the pairing half and the engine row half, outer on
  both sides. A missing half is not an error. See `deviceList.ts`.
- **Revoke and Remove.** Revoke is machine-global and Remove is per workspace,
  so they are two buttons.
- **Access.** `MobileAccessPage` renders the whole page, including the engine's
  Network access section. Its "Local network is off" row scrolls there instead
  of deep-linking. The guide and the Network access section each fetch `GET
  /api/v1/network-config` on mount, as two independent `Loadable`s.
- **Judge model link.** A link points to the judge model's row under Models.
  Neither the guard section nor the coding-agent auto-answer section owns the
  picker. The link carries no anchor.
- **Links.** One section answers where a link opens when you tap it. It has one
  platform-conditional row per answer. The external-link target row shows on an
  installed iOS PWA. The in-app browser row shows under Tauri.
- **Link override.** The per-link override is iOS's own long-press menu. We do
  not render it. The UI only names it.

## Rationale

- Pairing decides whether a device reaches the machine at all, on every
  workspace. The engine row decides where push goes and which preferences
  apply, here. Two lists under two words made the same phone read as two
  different devices.
- Revoke cuts a device off every workspace at once. Remove forgets its push and
  preferences in this workspace and leaves it paired. Those are different acts,
  so they get different buttons.
- The two `network-config` fetches read disjoint fields. The guide uses
  `gateway_bind` and `detected_tailscale_ip`. The bind editor uses
  `engine_bind` and `inherit`. The call is a cheap local one.
- The judge runs both the guard and coding-agent auto-answer. Search finds the
  picker itself, so the link needs no anchor.
- `Links` and `Experimental` used to be separate categories, each gated so it
  vanished off its platform. The external-link setting sat inside
  `Experimental`, whose nav entry was `isTauri()`-only. It shipped unreachable
  on the one platform it applies to.
- Gating now lives on the rows. The category above, Appearance & Behavior,
  renders Theme and Typography unconditionally, so no page can come up empty.
- Suppressing iOS's menu to draw our own would cost Copy Link, Share and Add to
  Reading List alongside Open in Safari. Naming the menu is the documented way
  to make it discoverable.

## Consequences

- A device with only a pairing, or only an engine row, still appears in the
  list.
- Opening the Access page costs two requests for one payload.
- Disjointness of the two fetches is the invariant. If both ever read a common
  field, one side goes stale after the other's Save. The fetch must then be
  hoisted into a single owner.
- No Settings page can come up empty from platform gating alone.
- `__tests__/settings-nav-structure.test.ts` pins the Links structure.
- Without a registry, the user connects by typing a name.

## Alternatives considered

- Showing a failed registry as a `LoadableError`. Rejected, because it is not
  an error on this page.
- Separate lists for pairing and for the engine row. Rejected, because one
  phone read as two devices.
- A single button for Revoke and Remove. Rejected, because their scopes differ.
- Deep-linking the "Local network is off" row to the Network access section.
  Rejected in favour of scrolling.
- Hoisting the `network-config` fetch into one owner now. Rejected while the
  two readers use disjoint fields.
- Separate `Links` and `Experimental` categories with platform-gated nav
  entries. Rejected, because the setting was unreachable on iOS.
- Drawing our own per-link menu in place of iOS's long-press menu. Rejected,
  because it would lose Copy Link, Share and Add to Reading List.
