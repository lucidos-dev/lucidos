# 0418: A retired model is migrated to its successor, never left to provider routing

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

Google deprecated `gemini-3.5-flash` and routes every request for it to
`gemini-3.6-flash`. Lucidos kept offering 3.5 and kept calling it wherever an
install stored it: background model preferences, trigger pins, each thread's
remembered model. Those calls ran on a model nobody chose, at a price Google
does not state. One workspace was fixed by hand. Every other install was not.

Nothing recorded which model actually answered, either. `ContextCaptured`
stored only the model Lucidos asked for, so the reroute was invisible.

## Decision

When a provider retires a model, Lucidos migrates every install off it, to a
named successor. Provider routing is never the fallback.

- **The successor lives on the row**: `models.successor`. A row with one is a
  *retired model*.
- **One SQL function does a retirement**:
  `retire_model(old, successor, accepted_efforts)`. A deprecation is a
  migration with one line that calls it. It disables the row, sets the
  successor, and rewrites every live setting.
- **The router follows the successor** before it resolves a route, and logs the
  substitution. An id that arrives after the migration never reaches the
  provider.
- **Every chat reply's model is recorded** as `ContextCaptured.served_model`.
  The router logs a warning when it is not the id it sent.

What `retire_model` rewrites is ADR 0248's test: a field read back as a live
setting.

- `chat_model` and each `model_*` preference holding the id.
- The `model=value` preference lists.
- Draft compose selections and queued turns.
- The `model` on `MessageReceived`, `TriggerStarted`, `TriggerCreated` and
  `TriggerUpdated`.

A reasoning tier beside a moved id snaps to one the successor accepts. No other
preference is touched, even one that holds the same string.

## Rationale

**A retired id is a decision the provider made for the user.** The replacement
may behave differently, cost differently, or stop answering. Lucidos owns the
model choice (`docs/philosophy.md`, own the surface, rent the model), so it
names the replacement itself.

**A migration, not a startup pass.** It runs once per install, in order with
the schema, and a test can seed every store and run it again. A boot pass would
scan the events table on every start for a change that happens once.

**The router guard exists because the database is not the only source.** An
id still arrives from outside it: `LUCIDOS_MODEL`, a CLI or API write, a
plugin's shipped `trigger.toml`, a stale PWA. ADR 0248 rejected a read-time
alias for a re-spell, where the old id named the same model. A retired id names
a model that no longer exists, so sending it is the exact failure this ADR
prevents.

**Response events stay as history.** ADR 0248 rewrote them because a re-spell
names the same model. A retirement names a different one, so a rewrite would
falsify which model answered. The retired row stays, disabled, so its label
still resolves. `ContextCaptured`, summary tree nodes and saved contexts stay
too.

**SQL cannot read the Rust effort ladders**, so the function takes the
successor's accepted tiers as an argument. A test reads every
`retire_model` call in `migrations/` and holds its list equal to
`supported_efforts` on each of the successor's routes. The snap itself mirrors
`clamp_effort`, and the same test holds the two equal over the whole ladder.

## Consequences

- The next deprecation is one migration line, plus the successor's row if it is
  not registered yet.
- A retired row is never deleted. Past turns keep their label, and a late call
  still finds the successor.
- A preference written after the migration keeps the retired id until the user
  changes it. The router still sends the successor, so no call reaches the
  retired id.
- `ContextCaptured.model` keeps its meaning, the model asked for. A cost rollup
  that wants the billed model reads `served_model` when present.
- `served_model` is absent on a streamed proxy reply. Coding-agent rows already
  put the agent's reported model in `model`, so they omit it.

## Alternatives considered

**Leave it to provider routing.** Nothing to build. Rejected: the user runs on
a model they did not choose, at an unstated price, until the provider stops
routing.

**Register the provider's routing target (`gemini-3.6-flash`) as the
successor.** It matches what calls already reach. Rejected for this retirement:
the target is a compatibility choice, not a model anyone picked. 3.8 Flash is
newer, has the same price, was already registered with two routes, and was
already live-tested. A successor that is itself one deprecation from retirement
would mean a second migration soon.

**A Rust constant map, applied on every boot.** One definition in code, no SQL
function. Rejected: it rescans the events table on every start, and the SQL
column keeps the map where the API and the router both read it.

**Disable the row and stop there.** The picker no longer offers it. Rejected:
every saved setting keeps naming it, so every call still goes to the provider
under the retired id.

**Warn on a different model family only.** Simpler to state. Rejected: the
case this exists to catch, 3.5 Flash answered by 3.6 Flash, is the same family.
The check compares the served id with the exact id sent, allowing only a dated
snapshot or a local tag.
