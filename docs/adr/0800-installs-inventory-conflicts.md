# 0800: The installs inventory reports only port contention as a conflict and never runs or writes anything

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0189](0189-coexisting-installs-are-told-apart-by-port.md)

## Context

A machine can carry three install vehicles at once: the macOS `.app`, an
`install.sh` headless install, and a source checkout. Each writes its own
binaries, launch agent, data dir and gateway port.

None of them can see the others, and neither uninstaller knows the other
exists. This is a trap rather than a nuisance.

An `install.sh` install laid down first keeps port 5252. A DMG installed later
cannot bind it, so its gateway dies on every launchd respawn while the new
client drives the old engine. The user reads a current client version beside a
ten-release-old engine, and nothing explains it.

This record restates the design comments of
`crates/lucidos-installs/src/lib.rs`, written before the comment strip removes
them. Nobody checked it line by line against the code.

## Decision

The `lucidos-installs` crate lists every Lucidos install the user can see and
reports whether any two of them fight.

- `Inventory::conflicts` reports port contention and nothing else. Coexistence
  is supported and stays silent.
- The scan only reads. It never runs anything and never writes.
- Every version comes from a path or a plist, never from executing a discovered
  binary.
- Each field of an install is what the filesystem says, with `None` where it
  says nothing.
- The directories a scan reads are passed in, not resolved inside the scan.
- The app version is `CFBundleShortVersionString`, read from an XML
  `Info.plist` by a deliberately small reader. Any other form answers `None`.
- A helper returns plain lines describing every install the caller is not
  removing, for an uninstaller that cannot reach them.

## Rationale

- Running a source checkout beside the packaged app is an ordinary developer
  setup, and the two sit on different ports. The ports already say whether two
  installs fight, so no marker file, environment variable or setting is needed.
- The read-only rule is a safety property, not an optimization. The code runs
  at client startup and would otherwise execute whatever sits at a well-known
  path.
- A guess would be worse than a gap. The point is that the user can trust the
  inventory.
- Passing the directories in lets a fixture tree drive the whole scan with no
  environment.
- Apple's tooling and Tauri both emit the XML plist form, so a small reader is
  enough.
- An uninstaller that says done over a second engine is what turned one
  tester's afternoon into a day.
- Each leftover line names the install, its data, and the command that takes it
  away.
- The leftover helper takes a `removed` predicate rather than a kind. A vehicle
  is not always removed whole: the app's own uninstall trashes the running
  bundle, so a second bundle elsewhere is a leftover like any other.

## Consequences

- Coexisting installs on different ports raise no conflict.
- An install whose version cannot be read from a path or an XML plist shows
  `None` instead of a guessed value.
- The test `no_execute_and_no_write` pins the read-only rule by scanning the
  source file of the crate.
- Tests can build a fixture tree and scan it with no environment.
- An uninstaller can warn about installs it cannot remove instead of reporting
  done.

## Alternatives considered

- A marker file, environment variable or setting to declare coexistence.
  Rejected because the ports already say which case it is.
- Running a discovered binary to learn its version. Rejected because it would
  execute whatever sits at a well-known path at client startup.
- A full plist parser. Rejected in favour of a small reader for the XML form.
- Resolving the scanned directories inside the scan. Rejected so a fixture tree
  can drive it.
- A removed-kind argument for the leftover helper. Rejected for a predicate,
  because a vehicle is not always removed whole.
