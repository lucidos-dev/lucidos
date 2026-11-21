# 0531: The browser e2e suite runs on calm motion, and a spec runs on desktop Chromium unless its suffix asks for the phones

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

The browser suite ran every unsuffixed spec on three projects: desktop
Chromium, mobile Chromium and mobile WebKit. A full run took about 38 minutes
on GitHub, and the main check went red often for two reasons:

- **Timing flakes.** About 75 e2e commits in one month waited out a glide, a
  fade or a late frame.
- **Repetition.** 148 of 254 specs carried no suffix, so most logic ran three
  times, including on WebKit, the slowest leg.

Plan: `docs/plans/2026-10-19-e2e-efficiency.md`.

## Decision

1. **Calm motion.** `playwright.config.ts` sets `reducedMotion: 'reduce'`. A
   spec that tests motion opts back in with
   `test.use({ reducedMotion: 'no-preference' })`.
2. **One browser by default.** The file suffix picks the projects
   (`e2e/projectRules.ts`):

   | Suffix | Projects |
   |---|---|
   | none, `-desktop` | desktop Chromium |
   | `-mobile` | mobile Chromium, mobile WebKit |
   | `-all` | all three |

## Rationale

- Reduced motion is a supported product mode. The app scales every duration to
  near zero, so a spec that does not test motion loses nothing and stops
  racing it.
- Most unsuffixed specs test logic that does not depend on the layout or the
  engine. A third of the WebKit time bought no coverage.
- A suffix is visible in the file name. A reader, the shell scripts and the
  shard planner all decide from the name, never from a runtime skip.

## Consequences

- **Every phone or WebKit path keeps a project.** The rename audited each
  unsuffixed spec. 71 assert a phone or WebKit path, by a code branch or by a
  stated iOS or WebKit regression, and became `-all`. The rest lost their dead
  phone branches.
- Each phone project dropped from 648 listed tests to 426. Desktop Chromium is
  unchanged.
- **Three guards hold it.** All are Vitest tests under `e2e/`:
  - `motion-stance.test.ts`: a spec that reads motion names its stance.
  - `project-rules.test.ts`: the shell mirror `project_runs_spec` agrees with
    the config for every spec, and every spec runs somewhere.
  - `project-rules.test.ts`, again: a desktop-only spec carries no
    `isMobile`, `browserName`, `project.name`, `hasTouch`, `tap()` or
    `touchscreen` branch.
- **A new spec that should cover the phones must say so** with `-all` or
  `-mobile`. The branch guard catches the common shapes, not a phone-width
  `test.use({ viewport })`, which still runs in desktop Chromium.
- GitHub shards follow the work: 7 desktop Chromium, 5 per phone project.

## Alternatives considered

- **Drop mobile Chromium.** Separate decision. It is the cheap phone layout
  check and keeps WebKit from being the only phone signal.
- **Default unsuffixed specs to all three, and add a `-chromium` suffix.**
  Every new spec would keep paying for three projects unless its author
  thought to opt out. The cost belongs on the spec that needs the phones.
- **Keep three projects and shard wider.** GitHub caps the public repo at 20
  concurrent jobs and macOS at 5, so wall time cannot drop that way.
- **A per-spec `emulateMedia` in a shared helper** for calm motion. The suite
  default is one line, and the opt-in stays visible at the top of the spec.
