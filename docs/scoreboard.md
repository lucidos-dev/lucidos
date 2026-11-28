# Delivery and app speed scoreboard

Run `./scripts/scoreboard.sh` from a Lucidos agent shell. Use `--days N` to
change the window from seven days. Use `--json` for the full report and lab
samples.

## Lab runs

Choose an idle build slot pool on a macOS or Linux source checkout. Use a
workspace CLI that records the build slot class. Each step checks the pool,
then takes a background slot. The runner records a deferral and exits with
status 75 when a slot is held or a build is waiting. The app lab also defers
when the required e2e slots are busy. A build that arrives
during an active step can take the next available slot.

- Run `./scripts/measure-engine-build.sh --record-lab --only noop_build` to
  measure a no-op engine build after a warm-up build. The build lab runs
  three passes and records one result for each step. Each pass keeps its logs
  and TSV.
  Choose other steps with the benchmark's existing flags.
- Run `./scripts/measure-app.sh chromium` or `./scripts/measure-app.sh webkit`
  with the corresponding Playwright browser installed. The script starts
  a disposable local e2e workspace. It seeds a synthetic 1,800-event
  transcript. It measures cold boot, a notification link to the middle of
  the transcript, and switching to that thread from another thread. It
  reads the posted app performance marks. Each browser has its own result.

The scoreboard puts the median beside its host fingerprint and sample count.
Compare runs with the same fingerprint, step, and configuration. For the app
lab, also match the browser version and seed size. Playwright WebKit results
apply to that browser engine. Use field samples for real iPhone measurements.
