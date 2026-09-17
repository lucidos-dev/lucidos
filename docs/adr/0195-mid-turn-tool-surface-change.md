# 0195: A mid-turn tool-surface change is re-sent at once, and the cache write is the price

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

A chat turn used to freeze its tool array at setup. The agent could call the
`mcp` tool, bring a server up, and read back "started with 15 tools available".
Not one of those tools was callable for the rest of the turn. Observed on a
packaged build: the agent told the user that MCP tools only register at the
start of a turn, and deferred the query to the next message.

ADR 0088 part 4 rejects **lazy tool disclosure** on exactly the mechanic this
fix needs. The tools array is the FIRST cache segment. Changing it forfeits
every tier behind it, which that measurement priced at $0.71 per fetch against
a $0.0095 saving per call.

The two are not the same act, and the tree had no record saying so.

## Decision

A change to the MCP tool surface reaches the very next round of the running
turn. `McpManager` carries a generation counter, bumped by every mutation that
moves what it offers, and the agentic loop compares it once per round. On a
move it rebuilds the MCP slice and pays the cache write. On no move it does
nothing, which is every round of almost every turn.

## Rationale

**The cost ADR 0088 priced is per *fetch*, and this is not a fetch.** Lazy
disclosure would have changed the array as a routine way of naming tools,
around 110 times a week on the measured workspace. A server start is a rare,
deliberate act by the agent or the user. The engine pays one cache write for
it, once, at the moment the capability actually changed.

**The alternative is not a cheaper turn, it is a dead one.** Without the
refresh the turn carries an array that no longer describes the workspace, and
every tool the agent just enabled is unreachable. The user pays a whole extra
round trip, and the agent has to explain an engine internal to them.

**A generation beats a dirty flag.** A boolean cannot answer "changed since
when". A turn that started before the flag was set and cleared reads false and
stays stale. Two concurrent turns cannot share one flag at all. The counter is
read under the same lock as the tools it stamps, so a turn holds a stamp that
describes the array beside it.

**Only the MCP slice moves.** ADR 0088 part 2 makes the engine-authored
families a function of workspace configuration, never of the thread, so
re-deriving them per round would change nothing. Leaving them in place and in
order keeps the head of the array stable. That is the half of the cache prefix
this change never touches.

**Stop is the same decision as start.** A server the user stopped mid-turn
stops being offered in the same round. The dispatch gate already refuses a call
to a stopped server (`dispatch_refusal`), so the two now agree: the array says
what dispatch would allow.

## Consequences

- The round after an MCP start, stop, remove or disabled-tools change pays a
  full prompt-cache write. Nothing else in a turn changes the array.
- The message budget moves with the array, so a server that adds 15 schemas
  leaves the trimmer correspondingly less room in the same turn.
- `ContextCaptured` reports the array of the round it describes, not of round
  one. A Context Viewer reading of a turn that started a server shows the
  schema count changing between rounds, which is what was sent.
- Any future surface that wants to change the array mid-turn has this as its
  precedent, and ADR 0088 part 4 as its limit: a capability the user or the
  agent genuinely changed, not a disclosure mechanism.

## Alternatives considered

**Rebuild the whole array every round.** Rejected: it re-derives capability
gates that cannot move mid-turn, and it invites the array to differ round to
round for reasons nobody decided. The generation check costs one atomic load
and states exactly what may move.

**Have the `mcp` tool handler push the new tools into the live array.**
Rejected: it puts knowledge of the turn's tool array inside a tool handler, and
it covers only the agent's own route. The user starting a server from Settings
during a turn would still be invisible. The manager already owns every route.

**Leave it, and tell the model to ask the user for another message.** Rejected:
that is the observed behaviour, and it is the engine's limitation surfacing as
the agent's advice. The prompt cannot make a missing tool callable.

**Restart the turn after a surface change.** Rejected: it throws away the
round's work and every tool result in it, to save one cache write.
