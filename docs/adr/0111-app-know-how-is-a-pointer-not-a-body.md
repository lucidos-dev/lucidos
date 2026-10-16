# 0111: App know-how is a pointer in the active app block, not a body

- **Status**: Accepted
- **Date**: 2026-08-23

## Context

The `[ACTIVE APP UI]` block tells the chat agent which app the user has open.
It used to append every `data/apps/<id>/knowhow/*.md` BODY, tagged
`[APP KNOW-HOW: name]`. The block is unconditional and rebuilt on every round
of the agentic loop, whether or not the turn is about the app.

Measured from `ContextCaptured`, section `App Context`, over 30 days on the
workspace that motivated this: 4,625 rounds under 40k chars, 798 rounds between
40k and 100k, 466 rounds at 100k and over. The peak was 136,065 chars, from one
app with one doc of 135,475 bytes. That is about 47% of the 288,000-char budget
of a 200k-token model, spent before the user's question is read.

It was also billed twice. An app-scoped doc already appears in the system
prompt's Know-how routing list with its id and description, as
`<app>/<doc> (app: <app>)`, and `load_knowhow` resolves that id. 488 rounds in
30 days carried the same doc in both homes, 22.7M chars.

## Decision

The block keeps the pointer and drops the bodies. It still names the open app,
its files path and the `DEFAULT TO THIS APP` anchoring clause. In place of the
bodies it lists the app's docs by id, name and description, one line each. A
sentence above them says to call `load_knowhow` with an id when the turn needs
it.

The listing is built by `build_app_knowhow_listing` in
`engine::chat::process::workspace_payload`, from
`KnowhowStore::load_summaries`. That is the same loader the routing list uses,
and both print the id through one `app_scoped_id` helper.

## Rationale

The body was never the cheapest way to make the doc reachable. It was one of
two copies of the same text, and the other copy was already addressable.

Reuse of `load_summaries` is the load-bearing part. Two surfaces now name an
app's docs. An id printed in one that the other cannot resolve is an agent sent
to a doc that does not answer. Deriving both from the same loader, through the
same id helper, makes that divergence a test failure rather than a silent
misroute. Two tests parse the ids back out of the rendered block and assert
them against `load_with_fallback` and against the routing list.

A description is rendered through `routing_description`, the same 400-char
render-time ceiling the routing list uses. So a user who writes an essay into
`description:` cannot reintroduce the cost the bodies used to carry.

Measured before and after for the assembled block:

| The app's know-how | Before | After |
|---|---|---|
| 1 doc, 135,475 bytes | 136,065 | 1,699 |
| 1 doc, 20,446 bytes | 20,733 | 1,728 |
| 1 doc, 2,509 bytes | 3,225 | 1,716 |

The three apps on that workspace that have know-how at all, largest first. The
after column is flat because it prices the doc COUNT, which is one in each case.

## Consequences

**The size rule is now a property, not a number.** The rendered block must stay
a function of how MANY docs the app has, never of how big they are. A test
renders the same doc with a 1-char body and a 140,000-char body and asserts the
two blocks are the same length. Anyone widening this block owes that test.

**The agent takes one extra round trip** when a turn genuinely needs an app
doc. Every other knowhow doc in the workspace already worked that way. The
round trip is now paid by the turns that need it, rather than by all of them.

**`load_app_knowhow` stays.** `handle_execute_intent` still appends an app's
bodies to the isolated intent sub-loop's prompt. That is a sub-loop the user
explicitly asked for, not every round of every chat turn, so the economics
differ. The double-billing there is known and deliberately left alone.

**A local file still overrides an app doc.** `load_with_fallback` tries
`data/knowhow/` before the app-scoped path, so a workspace file at
`data/knowhow/<app>/<doc>.md` answers to the id the app block prints. That
priority predates this change and is the documented override. It means the id
in the block is a promise about resolution, not about which file answers.

## Alternatives considered

**Keep the bodies, add a size cap.** Rejected. A cap turns a complete doc into
a truncated one, which is worse than a pointer: the agent cannot tell what it
is missing, and the doc it half-read is the one it will answer from. The same
reasoning ADR 0086 applies to the file listing, an inventory or nothing.

**Keep the bodies, but only when the turn looks app-related.** Rejected. That
is a classifier standing between the agent and its context, and it fails in the
expensive direction: the turns where the app doc matters are exactly the ones a
keyword test misreads. `load_knowhow` is the agent making that judgment itself,
with the description in front of it.

**Drop the bodies and add nothing.** The routing list already carries every one
of these docs, with the same id, name and description. So the app block's
listing is strictly redundant with it.

Rejected anyway. The routing list is flat and workspace-wide. The app block's
value is that it says which entries belong to what the user is looking at. The
listing cost about 550 chars on the workspace measured here.

**Give app knowhow its own new loader.** Rejected as duplication. The sibling
already existed: `load_summaries` reads the same frontmatter that
`load_app_knowhow` parses, and it is what the routing list is built from.
Adding a second reader of the same files is how the two surfaces would have
drifted.
