# 0490: The stable sections of the turn context ride in a marked block ahead of the volatile rest

- **Status**: Accepted
- **Date**: 2026-10-16

Record: `docs/plans/2026-10-16-stable-first-turn-context-block.md`.
Extends [ADR 0084](0084-volatile-values-never-enter-the-cached-system-tier.md) from the
system block to the user message, and
[ADR 0362](0362-tree-memory-module-and-the-home-thread.md) with one more block
in its request order.

## Context

A chat or trigger turn sends one large user text block. Sections that change
every turn sat early in it, so a warm turn boundary re-wrote almost the whole
block at 1.25 times the input price.

Anthropic's cache writes an entry only at a marked block boundary. A read finds
an entry only at a block boundary, at most 20 blocks before a marker, with every
byte before it identical. A read never goes partway into a block.

Fourteen days of dev data showed the cost. A warm Home turn read 53.7% of its
input from the cache, a warm chat turn 49.8%, and a warm trigger fire 20.9%.

## Decision

**Split the per-turn text into a stable context block and the rest.** The stable
context block holds, in order: the user profile, the email accounts, the OAuth
accounts, the thread context and the loaded knowhow bodies. Every other section
stays in the volatile text in its old relative order, with the request and the
tail last.

**Knowhow retention notes leave the knowhow block.** `[LOADED KNOWHOW]` keeps
the bodies, each doc's id and the general rule. A small `[KNOWHOW RETENTION]`
block in the volatile text carries one countdown per doc. The unload rule does
not change.

**The stable block is a content block of its own.** `ContentBlock::StableContext`
is plain text on every wire, and only the Anthropic wire marks it.
`TurnMemoryViews::lead` orders a message's leading blocks for both memory
modules.

- **Tree:** `[snapshot][stable][thread view][volatile]`. The snapshot stays
  first, so every thread still shares it. The thread view's rungs read the
  stable block through their own prefix, so it needs no marker of its own.
- **Classic:** `[stable][history and volatile]`, with the stable block marked.

**Marker priority** is now the last message, the penultimate message, the memory
view blocks, the system block, the stable block, then the last tool. The tools
marker gives way, because the system entry's prefix already holds the tools.

**History stays unmarked.** The Classic history is rebuilt every turn, so a
marker on it would be a write nobody reads.

**A trigger fire gets the chat turn's system block.** `TRIGGER_SYSTEM_ADDENDUM`
moves into the static prompt body for every turn and stays system-tier. The
firing trigger's own know-how listing moves into the fire's volatile text as
`[TRIGGER KNOW-HOW]`.

## Rationale

The block cannot be partly read, so a section earns a place in the stable block
only when its bytes hold from turn to turn. Measured over 154 warm turns, the
profile changed in none, and the loaded knowhow set changed in 10. The device
line, the file list, the credentials text, recall and the history change per
turn or per message, so they stay out.

The knowhow bodies are the largest section, at about 82k characters. Their
retention note ticked every finished turn, which alone forfeited most of the
saving. Moving the note costs one small volatile block and no behavior.

A trigger's cost sits in its tools and system block, about 69k tokens of input
per fire. Fires run in their own one-turn threads, so a fire can only read a
prefix that an earlier request wrote. Two fires with the same model and tool
array now share the tools, system, snapshot and stable entries, whichever
trigger wrote them.

## Consequences

- Warm Home and chat turns should read roughly 23k to 24k more tokens from the
  cache. A fire that matches a fire from the last five minutes should read about
  45k. The plan holds the measurement query and the baseline.
- A turn where the knowhow set changes (about 7% of turns) misses the stable
  block, and on Tree it also misses the thread view.
- A Classic turn reads its stable block only after a tool-free turn, or under
  context mode, because resumed tool blocks sit before the user message.
- The always-loaded ceiling grows by the 975 characters of the trigger addendum.
- `anthropic_wire_classic_request.json` pins what the wire writes for a fixed
  input. It does not pin the assembled message, which now opens with a stable
  block.

## Alternatives considered

- **A 1-hour TTL on the trigger prefix.** Within an hour 397 of 431 fires had a
  matching earlier call, against 209 within five minutes. ADR 0470 priced and
  rejected the 1-hour TTL, and Vertex support is unproven. Rejected.
- **Per-trigger MCP tool arrays.** Two fires with different MCP servers share no
  entry. Fixing that means changing how ADR 0430 offers MCP tools. Deferred.
- **A workspace-wide stable block ahead of the snapshot.** It would let a new
  thread read the profile across threads. It is worth about $4 per 14 days in
  dev and costs a second block kind and a second ordering rule. Rejected.
- **A marker on the Classic history.** It cannot be read at a turn boundary in
  its current form. Caching it needs per-message blocks with rungs, plus a fix
  for the per-turn image ages. Deferred.
- **Marking the stable block on the GPT wire.** Deferred.
