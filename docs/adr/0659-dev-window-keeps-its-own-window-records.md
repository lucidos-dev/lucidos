# 0659: The dev window keeps its window records in the checkout and restores every window after a switch

- **Status**: Accepted
- **Date**: 2026-11-11
- **Amends**: ADR 0123 ("Dev writes nothing"), ADR 0461 (the relaunch page)

## Context

After "Switch to new version" from the Tauri dev window, two things went wrong.
The window lost its size, and a second window on a remote workspace closed and
never came back.

- **The size.** The dev binary embeds `tauri.conf.json`, so its identifier is
  `com.lucidos.app`, the same as the packaged app. `tauri-plugin-window-state`
  writes `app_config_dir()/.window-state.json`, one file for both clients. Each
  client writes its whole cache, so the relaunched dev window wore the packaged
  `main`'s frame.
- **The window.** ADR 0123 made dev write no session record, because dev shares
  the packaged app-data dir. So a relaunch started `main` alone.
- **The page.** ADR 0461's relaunch reopened on the page of whichever window
  answered the new engine first. That could be an extra window's page.

## Decision

The dev window keeps both window records in `<checkout>/.launch/dev-window/`.
`tauri-dev.sh` passes that dir as `LUCIDOS_DEV_WINDOW_RECORDS_DIR`. One
resolver, `window_persist::WindowRecords`, decides where records live:

| Client | Records |
|---|---|
| Packaged | the app-data dir, as before, and the plugin's default path |
| Dev with a records dir | that dir, for the session record and the plugin file |
| Dev without one (a bare `cargo run`) | nowhere: it registers no plugin and records nothing |

A legacy `LUCIDOS_NO_GATEWAY` launch gets `.launch/dev-window-no-gateway/`
instead (`dev_window_records_dir` in `scripts/lib/workspace.sh`).

Every launcher start restores the recorded arrangement through the packaged
code path, `desktop::launch_plan`, with the launcher's page as the tap. A switch
writes `main`'s page to the relaunch file, not the calling window's.

## Rationale

- **Dev must never touch the packaged files.** Two clients writing one file is
  a race neither can win. A dir inside the checkout cannot collide with the
  packaged app, any other checkout, or the OS app-data layout.
- **`.launch/` already holds the checkout's launch state.** It is gitignored,
  and `cargo clean` never reaches it (ADR 0063).
- **One path for both clients.** Dev restores through `launch_plan` and
  `restore_extra_windows`, so a fix to packaged restore also fixes dev.
- **`main`'s page as the tap keeps `main`'s frame.** `launch_plan` drops the
  record's first frame when the tap is on another workspace. With `main`'s own
  page, `main` keeps its frame and each extra comes back once.
- **Restore on every start, not only after a switch.** A packaged launch does
  the same, and no "was this a switch?" flag is needed.

## Consequences

- A dev "Close to Menu Bar" is now a teardown. It records the arrangement and
  calls `begin_teardown` before it closes the windows, so the record never
  shrinks to the last window.
- The launcher, not the URL, separates the launch modes. A no-gateway window
  sits on a bare engine origin and records no workspace windows. In a shared
  dir its first save would empty the gateway window's arrangement. A URL test
  could not tell that origin from the gateway's picker, and a switch from the
  picker lost every extra window.
- The plugin's exit save ignores its filter, so a process with no records dir
  must not register the plugin at all.
- The plugin creates its app-config dir before every save, even with an
  absolute filename. So a dev window can recreate an empty
  `~/Library/Application Support/com.lucidos.app/` after a packaged "Delete
  Everything" on the same Mac. It writes no file there.
- The first dev start after this change opens at the declared size, once.
  Nothing migrates from the shared packaged file.
- A bare `cargo run -p lucidos-app` remembers nothing between runs.

## Alternatives considered

- **A dev-only bundle identifier.** It would move both records at once. But
  `tauri.conf.json` is the packaged config, and a second config would fork the
  plugin set, capabilities and deep-link setup for one path.
- **Keep "dev writes nothing", and gate only the plugin.** It fixes the size
  and loses every extra window on each switch, which was half the bug.
- **Restore only after a switch.** It needs a flag the launcher passes, and
  makes a dev start behave unlike a packaged one.
