# 0758: Lane catch-up certificates

- **Status**: Accepted
- **Date**: 2026-11-21

## Context

[ADR 0662](0662-closed-lane-threads-stay-closed.md) requires a fresh full
certificate before landing an integration head. A clean catch-up used to
invalidate that certificate even when `main` only changed unrelated files.

## Decision

With main-check coverage, carry a fresh full certificate across a clean
catch-up whose incoming paths are disjoint from the batch. Commit the marker
update and `HardenCertificateCarried` together. For a clean overlapping
catch-up directly on a full marker, scope review and suite selection to the
overlap. A conflict, an unknown, or absent coverage keeps the ordinary gates.

## Consequences

A semantic interaction across disjoint files can first surface after
landing. The main check owns that risk and starts a main repair on failure.
