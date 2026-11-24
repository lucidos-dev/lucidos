# 0714: Assemble DMGs without Finder automation

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

Tauri's DMG bundler asks Finder to arrange the mounted volume through AppleScript.
Finder rejects that request inside the agent sandbox, blocking release builds.

## Decision

Ask Tauri for the app bundle only. Assemble the DMG from the final app with
`hdiutil`, generating its `.DS_Store` using dependency-free Python.

## Rationale

The shipped layout contains five records and no background image alias.
Writing those records directly preserves the window without desktop automation.
Terminal builds and sandbox builds use the same path.

## Consequences

Keep the shipped window bounds, icon positions, volume icon, naming, HFS+,
GUID partition map, and UDZO compression at zlib level 9.
Build around the signed and stapled app before signing and notarizing the DMG.
Tests mount a real image and read back its layout and Finder flags.
The allocator bytes differ, while the layout records match the shipped image.

## Alternatives considered

- Tauri CI mode skips Finder but also loses the window layout.
- A Finder broker widens the sandbox boundary for avoidable desktop automation.
- A checked-in `.DS_Store` obscures the settings and adds a binary to maintain.
