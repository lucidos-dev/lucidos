# 0413: The workspace prompt footprint is measured by the engine and caught by the audit, not capped by design

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

Every Lucidos Agent turn carries sections sized by what the user keeps. The
Available Apps list, the reusable widgets, the Know-how routing list and the
Available Intents grow by one line per item. So do the email, OAuth and
credential lists and the stopped MCP servers. The user profile and the running
MCP servers' tool schemas grow with content.

Seventeen such parts exist. Each item description is cut at render time, but no
section caps its item count. `ALWAYS_LOADED_BUDGET_CHARS` meters only the text
Lucidos writes, and leaves this half out on purpose (it is the *system prompt
footprint*). The one test over this half measures a synthetic fixture, so it
says nothing about a real workspace.

One development workspace measured 33 apps at 7,356 chars and 44 knowhow docs
at about 21,000 chars, on every turn. The widgets design thread then chose to
put every reusable widget's params in the prompt too, because a user rarely
keeps many. That choice is safe only if something notices when a user does.

## Decision

The engine measures the *workspace prompt footprint*: every workspace-grown
section, built by the same builders a turn uses. A route and a CLI command
serve it. The *workspace audit* gains a prompt-footprint *audit section*. It
flags a section past one ceiling and a total past a second. It also lists
clipped descriptions and items unused for N days. A Settings page shows the
same report.

The prompt itself stays uncapped by count. The audit catches the growth, and
the user decides what to trim.

## Rationale

- **A count cap narrows what the agent can reach.** An app missing from the
  list is an app the agent cannot open by name. Lucidos already refuses to send
  a sample of the file listing for the same reason (ADR 0086 as amended).
- **The user owns the trade.** Thirty apps may be worth their tokens to one
  user and waste to another. A ceiling the user can move in preferences says
  so, where a hard cap decides for them.
- **One definition of what a section costs.** The report calls the turn's own
  builders. An audit that recomputed sizes in bash would copy the truncation
  rules and line formats, and drift the first time a builder changed.
- **Measuring the prompt must not grow it.** The audit reaches the report
  through the `lucidos` CLI it already runs, so no tool schema is added. The
  audit sections are knowhow references, kept out of the System Knowhow
  routing list.

## Consequences

- Three agent-writable preferences hold the numbers: a section ceiling (6,000
  chars), a total ceiling (20,000 chars) and an unused window (60 days). The
  preference catalog is their one definition.
- A new stored `AppOpened` event records a real open of an app UI. Without it
  the audit could not tell a used app from a dead one. Opens before the event
  existed are unknown, so no app is judged unused until recording has run for
  the full window.
- The audit's remediation splits by reversibility. "Do all suggested fixes"
  covers shortening a description, stopping a widget's reuse and moving a
  rarely loaded doc under a group doc. Deleting an app or a knowhow doc always
  needs its own confirmation naming the item.
- The workspace audit splits into audit sections, each a knowhow reference
  that runs on its own. Shipped knowhow below the top level becomes a
  reference. Its doc names it, and the routing list does not, which is how a
  workspace root already treats one.
- The dev-only term *workspace payload* retires into *workspace prompt
  footprint*, renamed in every layer.

## Alternatives considered

- **Cap each section's item count.** Rejected: it narrows reach, as above, and
  any number is wrong for someone.
- **Load the lists on demand.** The widgets thread weighed this and rejected
  it: a list the agent never sees is a list it never routes to. This ADR keeps
  that decision and makes its cost visible.
- **Split the System Instructions row of `ContextCaptured` and read the newest
  capture.** Rejected as the source: it needs a recent turn, and gated blocks
  come and go with each turn's classifier. The footprint reports the worst
  turn.
- **Recompute sizes in the audit's bash scan.** Rejected: a second definition of
  every line format.
- **A new agent tool for the report.** Rejected: its schema would bill every
  turn of every workspace to measure the prompt.
- **Infer app use from `navigate_ui` calls.** Rejected: an app the user taps
  daily but never asks about would read as unused, and the audit would suggest
  deleting it.
- **One ceiling only, total or per section.** Rejected: a per-section ceiling
  lets ten sections just under it pass, and a total alone does not say which
  section to fix.
