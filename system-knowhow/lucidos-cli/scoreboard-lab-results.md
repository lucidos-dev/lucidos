---
name: Scoreboard lab results
description: Build and app lab result events, deferred runs, and host fingerprints. Load when reading lab results or wiring a script that consumes them.
---

<!-- domain-event: BuildLabRunCompleted: The build lab records the median and samples for a step, or the next step when the run defers. -->
<!-- domain-event: AppLabRunCompleted: The app lab records the browser journey samples, or the next step when the run defers. -->

# Scoreboard lab results

`BuildLabRunCompleted` records one build step. `AppLabRunCompleted`
records three browser journeys. Read these workspace events with
`scripts/scoreboard.sh --json` from a Lucidos agent shell in a source checkout.

Check `status` before reading the payload. A completed result has the median
and samples for each measurement. A deferred result names the next step.
Compare numbers only when their host fingerprints and scenario settings match.
See [the scoreboard guide](../../docs/scoreboard.md) for runner commands.
