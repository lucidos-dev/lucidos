# 0526: The model and effort picker lives in the prompt bar as one rung control for both agent kinds; a coding agent lists only ladder rungs; a cache-rebuilding switch confirms first

- **Status**: Accepted
- **Date**: 2026-10-19
- **Amended by**: [0525: Chat intelligence ladder and price table](0525-chat-intelligence-ladder-and-price-table.md). The chat ladder gives the Lucidos Agent rungs, so its control lists them and steps them too.

## Context

The coding-agent model picker sat in the `/` menu. It listed every row Claude
Code reports plus the curated rows it omits, so old models and `[1m]` twins
stood beside the three current ones. The *intelligence ladder* (ADR 0500)
already says which model and effort pairs are worth offering. The owner asked
for the picker to offer only those. A model switch inside a live thread also
rewrites the messages cache, a cost the old picker never showed.

## Decision

The picker becomes the *rung control* in the prompt bar, beside Send. A coding
agent lists only rungs that are on. It also shows Default, the thread's own
pair when that sits off the ladder, and a link to the ladder last. The Lucidos
Agent gets the same control and opens the model registry's list, since it has
no ladder. A mid-thread pick that rebuilds a warm cache asks first, with the
estimated extra cost.

## Rationale

- One row sets model and effort together. A pair on the ladder is already a
  judged trade-off, so the list needs no old models.
- The prompt bar is where the user starts a thread and where the cost of the
  pick shows. Compose and a started thread show the same pill beside the
  ordinary Send button, and its two ends step one rung back or on. The list
  opens centred on the pill, scrolled so the rung in force sits mid-panel.
  The pill leads the right-hand cluster, so the action keeps the right edge. An earlier split "Start" button named the rung on the send itself. It
  went because the action button then moved between compose and a thread.
- One shared component serves both agent kinds, so the two look and shed the
  same way on a narrow bar.
- The confirm prices the switch from the thread's context size and the model
  prices. The cost is a one-time re-read, so the copy never says the thread
  runs slower.
- A coding-agent thread may hand off to a new thread on the picked rung. The
  Lucidos Agent has no successor flow, so its confirm offers Keep and Switch
  anyway only.

## Consequences

- The `/` menu has no model concern. The shortcut for the list presses the
  rung control's trigger.
- The chat estimate compares model id and effort only. A provider-only change
  on a chat thread is silent. The frontend skips the estimate when the pick
  equals the shown pair.
- Codex has no slash commands, so its `/` button stays dimmed and inert.
- A handoff from the confirm has no message of its own. It takes the composer
  text, else a stock opening line.
- A compose pick belongs to the draft and never writes an account preference.

## Alternatives considered

- **A global step shortcut** (one chord to move a rung up or down). Every arrow
  chord is already taken, so any choice would shadow another shortcut.
- **Keep the picker in the `/` menu and filter it.** The menu is the wrong
  place to see a cost, and the Lucidos Agent would still need its own picker.
- **Warn on every switch.** A same-pair pick and a cold cache cost nothing, so
  the confirm would only teach the user to dismiss it.

## Amendment: no Default row

The list no longer carries a Default row. While nothing is picked, the ladder's
default rung wears the checkmark in its own place in the list, so one rung is
never listed twice. A default pair the ladder does not hold is added as an
off-ladder row. Picking a rung always commits that pair.
