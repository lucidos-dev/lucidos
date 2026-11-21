# 0494: A plugin release asks for setup to run again with a setup_revision counter that must rise, never a boolean

- **Status**: Accepted
- **Date**: 2026-10-16

## Context

An update used to spawn a *setup thread* only when the new `setup` text differed from the installed one. Some plugins ship files whose real install step lives outside `data/`, such as a Claude Code skill that setup copies to `~/.claude/skills/`. A release that changes only those files leaves `setup` unchanged, so no thread starts and every user's copy goes stale without notice. Authors worked around it by rewording `setup`, which turns the instructions into a version stamp.

## Decision

The manifest gains `setup_revision`, a positive integer. An update re-runs setup when the staged revision is **greater** than the installed one. A missing field counts as 0. An optional one-line `setup_reason` says why. One function decides, at staging for the panel and at confirm for the spawn.

## Rationale

A counter the author raises cannot go stale. A value carried unchanged into the next release does nothing, so the re-run fires once, on the release that raised it. "Greater" rather than "differs" means removing the field or rolling back a release never re-runs setup. The decision records its cause on the origin, so the seed, the panel and the popover each say why setup runs.

## Consequences

- Adopting the field is itself a raise: going from absent to `1` re-runs setup on that update.
- The setup knowhow branches on the seed. A raised revision leaves the text diff empty, so the agent re-runs the steps that install shipped files and does not re-interview.
- The authoring procedure in `system-knowhow/plugins.md` asks the publisher to decide on every release.
- Old stored origins decode unchanged, as *instructions changed*.

## Alternatives considered

- **A boolean `needs_setup`.** Left `true`, it re-runs setup on every later update. Left `false`, it says nothing. Either way the author must remember to flip it back, and forgetting is silent.
- **"Differs", not "greater".** A rollback or a removed field would re-run setup, which asks the user to repeat work for no new reason.
- **Rewording `setup` to force a re-run.** It works today, and it is the problem: the instructions become a version stamp, and users re-read steps that did not change.
- **Hashing the shipped files outside `data/`.** The engine cannot tell which files setup installs elsewhere, since it never interprets `setup`.
