# 0414: Apps and widgets show an image icon named by the app manifest, with a monogram tile when missing; plugins ship media kept per install, and reusable widgets; no plugin code runs before install

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

While designing widgets inside question-card options, the user noticed that no
surface shows an app's own picture. A check of the code confirmed it:

- The apps panel, the menu drawer's pinned apps, the widget shelf, the widget
  card bar, Search Everywhere and the Plugins panel draw no per-app picture.
  The shelf and the card draw one generic widget glyph for every widget.
- The app manifest `icon` is a free string that nothing validates. The docs
  call it "emoji or asset path" and the tests use names like `"thermometer"`.
  Only the coding-agent App row renders it, as text.
- No app in the maintainer's workspace sets it, so the field is dead in
  practice.
- A plugin carries no picture of any kind, and its root rejects any entry
  besides `manifest.toml` and the content folders.
- `system-knowhow/plugins.md` says "Widgets do not ship."

The grill in the planning thread settled each branch.
Plan: `docs/plans/2026-10-10-pictures-for-apps-plugins-widgets.md`.

## Decision

**App icon.**

- An app or widget's icon is an image file inside its own folder, named by the
  app manifest: `"icon": "assets/icon.svg"` (SVG, PNG or WebP). The field has
  that one meaning.
- Without a valid icon, a **monogram tile** stands in: the first letter of the
  name on a tile coloured from the theme by a stable hash of the id.
- The icon renders as `<img>` in full colour, on every surface that names an
  app or widget. That covers the apps panel, the menu drawer, the widget shelf
  and card, Search Everywhere, the history menu and the coding-agent App row.
- The menu drawer's fixed rows get glyphs too, so an icon never stands alone.
- The Lucidos Agent writes an icon by default for every app and every widget.

**Widget shelf.**

- A chip shows its icon and label while the shelf has room.
- When the shelf is tight, a chip shows its icon alone, with the label in its
  tooltip.
- A per-instance chip shows its widget's icon, and the embed's alt text is its
  label.

**Plugin media.**

- A plugin may ship a top-level `media/` folder, which is never merged into the
  workspace's content.
- The plugin manifest names an `icon` and ordered `screenshots` and `videos` in
  it. `media/README.md` is the long description.
- A marketplace scan keeps each listed plugin's media in a `.lucidos/` cache.
- Install copies it to `data/plugin-media/<id>/`, an engine-managed gitignored
  path. So it outlives the marketplace and is in every backup.
- A Plugins panel row shows the short description and at most one screenshot.
  It opens the **plugin detail page**, with every screenshot and video and the
  rendered README.
- A plugin row's icon falls back to the icon of its single app, then to the
  monogram tile.

**Plugin widgets.** A plugin may ship widgets. Such a widget is always
reusable, and its app manifest names its plugin (`origin_plugin_id`) instead of
a thread. Uninstall removes it, update replaces it, deleting a thread never
touches it, and it offers no "Stop reusing".

**No plugin code before install.** The detail page shows only media. No live
showcase, demo page or widget runs for a plugin that is not installed.

## Rationale

- **One meaning per field.** An icon that may be an emoji, a glyph name or a
  path makes every renderer branch, and the docs and tests already disagreed.
  An image file is the one form every surface can draw. Emoji costs nothing to
  drop, because no app uses it.
- **The monogram makes the change visible on day one.** Every existing app has
  no icon. A generic glyph would leave a row of pinned widgets as a row of
  identical chips. A hash-picked theme colour gives each app a stable identity
  and follows the theme, at no model cost.
- **`<img>` is the safe renderer for third-party SVG.** Scripts in an SVG never
  run inside `<img>`. Inline SVG would make a sanitizer security-critical, and
  a CSS mask would reduce every icon to a one-colour silhouette.
- **Widgets need icons more than apps do.** The shelf shows them side by side,
  so the user asked for an icon on every widget, not only on apps.
- **Media is presentation, not content.** Screenshots and videos describe the
  plugin and make sense only with it, so they never land among the workspace's
  apps and artifacts. They must still outlive the marketplace (the user's
  requirement). A gitignored path under `data/` survives and is backed up,
  without putting every video version into git history for good.
- **A widget from a plugin has no thread.** Ownership by origin thread cannot
  hold in another workspace. Plugin ownership gives it the plugin's lifecycle,
  which the install record already tracks file by file.
- **Consent comes before code.** A widget reaches everything an app reaches
  (ADR 0402). A showcase from an uninstalled plugin would run third-party code
  with workspace access before the user agreed to install it. Videos already
  show motion and interaction.

## Consequences

- `plugins.md`'s "Widgets do not ship" is reversed for reusable widgets that
  name their plugin. A thread-owned widget still does not ship. "Make app" stays
  the way to ship one as an app.
- The plugin root accepts `media/` beside the content folders. Media problems
  (a missing file, a file over its cap, a wrong type) never block an install.
  The detail page names them, so the plugin author sees them.
- A plugin installed from a zip archive keeps its media, because install copies
  it. A plugin installed before this change has no media until its next update.
- Existing `icon` values that are not a valid path (an emoji, a glyph name)
  render the monogram tile, and the workspace audit reports them.
- `data/` gains one gitignored top-level path, `plugin-media/`.

## Alternatives considered

- **Emoji or path in `icon`.** Rejected: two kinds of string in one field, and
  every renderer branches on which it got.
- **A fixed `icon.svg` with no manifest field.** Rejected: it fixes the format,
  and it leaves a dead `icon` field to remove.
- **A generic kind glyph as the fallback.** Rejected: identical chips on the
  shelf, which is the complaint that started this.
- **An image-model icon at creation.** Rejected as the fallback: a model call
  per app, and it still needs a fallback when generation fails.
- **A CSS mask or inline sanitized SVG.** Rejected: the mask loses colour, and
  the sanitizer becomes security-critical code.
- **A captured thumbnail or an embed-named picture per widget instance.**
  Rejected: captures go stale as live data changes and are unreadable at chip
  size, and the embed syntax would grow a field. The alt-text label tells
  instances apart.
- **Screenshots in a strip inside the row.** Rejected for a detail page, which
  holds every screenshot, video and the full description without stretching the
  list.
- **Long description in the manifest or the repo README.** Rejected: multi-line
  markdown in TOML is awkward, and a repo README is written for developers.
- **Media from the cache only, or fetched live from the git host.** Rejected:
  the user requires media to outlive the marketplace, and live fetches break
  offline.
- **Media committed to git.** Rejected: every screenshot and video version
  would stay in the workspace's git history for good.
- **A packaged thread widget whose origin install rewrites to the setup
  thread.** Rejected: deleting the setup thread would strand it.
- **A sandboxed demo page, or the real widget after install.** Rejected: the
  demo is a second copy for authors to maintain, and after install the user can
  open the app itself.
