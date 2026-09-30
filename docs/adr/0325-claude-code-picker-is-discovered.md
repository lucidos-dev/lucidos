# 0325: The Claude Code model picker is exactly what Claude Code's initialize reply lists

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

The Claude Code `/model` picker in Lucidos was a hand-kept file,
`runtime/cc_menu_options.json`, resynced after every model release. The file
drifted: its `sonnet` row claimed Sonnet 5.5 for a user whose settings pin
`sonnet` to Sonnet 5.

Claude Code answers an `initialize` control request with the models its own
picker offers, for the account, provider and model pins it runs under. The
Agent SDK's `supportedModels()` reads the same field. A probe against Claude
Code 2.1.280 returned six rows in about two seconds, with no prompt and so no
token cost.

That list lags model releases. Claude Code 2.1.280 predates Sonnet 5.5, so it
does not list it. Nor does it list Opus 5, Fable 5 or Opus 4.x, although it
accepts all of them through `--model`.

## Decision

The picker is exactly the list Claude Code's `initialize` reply names, once a
probe has succeeded. The curated JSON is only the fallback before that, and the
overlay that declares context windows.

## Rationale

- **It follows the user's setup.** Model pins, provider and plan all shape the
  list, and no file in this repository can know them.
- **It ends the resync chore.** A release reaches the picker when Claude Code
  lists it, with nothing to edit here.
- **The user chose it** over a merged list, accepting the lag below.

## Consequences

- A model Claude Code does not list is not offered, even one it would accept.
  A new model waits for a Claude Code update.
- A thread already pinned to a dropped model keeps running. Resuming does not
  consult the picker, and the chip names it through `STATIC_MODEL_LABELS`.
- `run_coding_agent` takes `model` as a string checked at the spawn. An enum
  could not track a per-workspace list, and billed every row on every chat
  request. The check accepts a row's value or its `resolvedModel`, from the
  same reply: a session reports the resolved id, and the thread records it.
- A cross-workspace spawn cannot see the target's list, so the sender passes a
  Claude Code model through. The target's Claude Code runs it or fails loudly.
- A failed probe waits an hour before the next, so a lasting failure does not
  spawn a cold Claude Code on every session start.
- Only the `models` array is kept. The same reply carries the signed-in
  account, which the typed rows drop before anything is cached.
- A probe runs the user's own Claude Code session-start hooks, as every
  session does.

## Alternatives considered

- **Discovered rows first, then curated rows Claude Code does not cover.**
  Keeps new releases and old pins selectable, but keeps a hand-kept list in the
  picker. The user chose discovered-only.
- **Keep the hand-kept file.** It drifts from the user's setup, and needs an
  edit per release.
- **Send `initialize` inside every real session** instead of a separate probe.
  No extra process, but it changes the session driver's stdin ordering. Deferred
  until the probe has proven the shape.
