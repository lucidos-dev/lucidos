# 0742: An unchanged integrated sub-thread report files without waking its parent

- **Status**: Accepted
- **Date**: 2026-11-19
- **Amends**: [0465](0465-integrated-sub-threads.md)

## Context

A follow-up turn can hand up the same integrated branch and report again.
Under `wake`, that starts a parent turn even when the parent has nothing new to act on.

## Decision

File a successful integrated sub-thread's identical report at the same hardened head.
Compare with the latest report from that child on the parent thread.
Require reply text from the current turn before treating a report as a repeat.

## Rationale

The parent still receives the report for its next turn. The current idle's
head and the report both have to match, so a stale hardening marker cannot
hide a newer commit.

## Consequences

An explicit wake request and the final wake for a batch take precedence.
A repeat files during an automation pause without asking permission to read it.
Changed report text or pending changes can still wake the parent at the same head.
If the engine cannot establish a match, it follows the normal wake rules.

## Alternatives considered

Matching just the head would miss a new decision or a different set of
pending changes. Matching text alone would miss a new commit.
