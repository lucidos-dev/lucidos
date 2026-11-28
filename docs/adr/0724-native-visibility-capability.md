# 0724: Detect desktop native visibility support at the command boundary

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

Desktop pages can load a newer frontend from a local or remote engine inside an older installed shell.
Such shells may lack the native visibility command or its ACL permission.
Treating that response as pending initialization blocks interactive requests and form reconciliation indefinitely.
The current native source already registers and permits the command.

## Decision

Discover native visibility support from the command response, independently of release numbers.
Only exact missing-command and ACL-denied responses for `get_native_window_shown` mean unsupported.
Track initializing, supported, unsupported and failed states separately.
Unsupported shells retain unknown visibility and previous eligibility, while keeping the shared claim protocol from ADR 0721.
Other seed or listener errors propagate; subsequent attempts retry initialization and share concurrent work.
Release listeners on failed initialization and teardown.

## Rationale

The command boundary reports the capability of the shell actually hosting the page.
Installed shell versions and frontend releases can move independently across engines.
Unknown visibility preserves the older protocol without inventing a shown state from focus or document visibility.
An unrelated IPC failure cannot authorize presentation.

## Consequences

Current shells retain authoritative hide/show, listener-before-seed ordering and protection against stale seeds.
Older shells can present forms and requests, but cannot exclude hidden windows through a protocol they lack.
Shared IndexedDB journals, Web Locks and receipts still coordinate presentation across legacy and current participants.
Browser and PWA eligibility stays unchanged.
Compatibility is permanent protocol support, with no release cutoff.

Chromium tests simulate native IPC while exercising the actual adapter and independent browser pages.
They do not certify packaged AppKit behavior.

## Alternatives considered

- Repeating the native ACL change cannot update an older installed binary.
- Comparing releases cannot prove command availability across independently updated clients and engines.
- Treating every IPC error as unsupported would conceal genuine initialization failures.
- Requiring installation changes would leave otherwise functional older clients blocked.
- Bypassing shared claims would reintroduce duplicate or lost presentation.
