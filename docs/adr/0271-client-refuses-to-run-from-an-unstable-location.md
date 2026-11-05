# 0271: The packaged client refuses to run from a disk image or an App Translocation copy

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A user opened the packaged app without moving it into Applications. Two
failures followed:

- **Update & Restart failed** with "Cross-device link (os error 18)".
  `tauri-plugin-updater` first renames the running bundle into a directory
  under `$TMPDIR`, and a rename cannot cross devices.
- **The engine became unreachable**, with threads stuck at Requesting. On every
  launch the client writes `current_exe()` into the launchd service plist. That
  pinned the always-on service to a path that vanishes.

Two locations cause both failures. A mounted `.dmg` is read-only and goes away
on eject. A Gatekeeper App Translocation copy is read-only and gets a fresh
random path on every launch. macOS translocates a quarantined app opened from
where it was downloaded, until Finder moves it.

## Decision

At launch, the macOS client classifies its bundle. It is unstable when the path
has an `AppTranslocation` component or the volume is read-only. Then the client
shows one native dialog telling the user to move Lucidos into Applications, and
quits. It never writes the service plist from there.

The updater also checks, before downloading, that the bundle is on the same
device as `$TMPDIR`. If not, it fails at once with the same advice.

## Rationale

**Refuse, don't warn.** A warning that lets the launch go on still pins launchd
to the vanishing path, and that pin is the outage. It would also overwrite the
plist of a working install in Applications, if the user had one.

**Read-only is the test for a disk image, not a `/Volumes` prefix.** A writable
external disk under `/Volumes` keeps the service working while it is mounted.
Refusing it would lock out a setup that works.

**The update check is the plugin's own precondition.** Comparing devices catches
both unstable locations and the writable external disk. It fails in a second,
not after a 100 MB download, and the user reads advice instead of an errno.

**Fail open on an unreadable mount.** A failed `statfs` judges the bundle on its
path alone. Refusing to launch on a failed syscall could lock out an app that
works.

**No migration.** Once the user opens the moved app, the existing "plist
changed, bootout and bootstrap" path rewrites the service to the new location.

## Consequences

- Nobody can try Lucidos straight from the `.dmg`. That is the usual shape for
  a Mac app with a background service.
- An install on a writable external disk launches, but cannot update in place.
  It gets the advice to move to the startup disk.
- The dialog explains; it does not move the app. See below.

## Alternatives considered

- **Move the app automatically**, the LetsMove pattern. Finding a translocated
  app's real path needs the private `SecTranslocateCreateOriginalPathForURL`.
  Relaunching it untranslocated needs the quarantine attribute stripped. That
  is more machinery and more risk than one drag the user can do.
- **Point `$TMPDIR` at the bundle's own volume** so the plugin's rename stays on
  one device. `set_var` is unsound in a threaded process, and a read-only source
  volume fails the rename anyway.
- **Warn at launch and carry on.** Rejected above: it keeps the outage.
