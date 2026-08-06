# Product Philosophy

**Always loaded** (no `paths:` frontmatter): this is a lens on *proposals*,
applied before any file is touched, and it governs design discussion and
suggestions as much as code.

Vision, mission, reasoning and worked cases:
[`docs/philosophy.md`](../../docs/philosophy.md).

## When this applies

Only to proposals that add a **surface** (a new place the user meets Lucidos) or
an **integration** (a new relationship with somebody else's product). It has
nothing useful to say about concurrency, build determinism, recovery or schema
work. Do not apply it there.

## The test

> **Does it bring work into the workspace, or does it export the agent out of
> it?**

Both halves are usually claimable, so the headline is shorthand for one
distinction: **ramps in, never rooms.**

A **ramp** reaches out to where the user already is and leads back in: a
notification rendered by the OS, a share sheet, a widget showing what a trigger
found, a link in an email, a data source pulled from somebody's cloud, an app
downloaded from a store. It holds no state and its whole payload is a way in.
Ramps are good and we want more of them.

A **room** is somewhere the user works *instead* of coming here: the transcript
lives there, the formatting is theirs, the history is theirs, and the workspace
is reduced to whatever their protocol can express. When a proposal reads as
both, room wins.

**We start inside their systems on purpose.** A notarized Apple `.dmg`, APNs and
FCM, reading the user's calendar and mail out of Google's and Apple's clouds:
all deliberate, because that is where people are. The clause that makes it
survivable is that **we own the fallback** (the headless tarball against the
DMG). Do not cite this page against a ramp.

## The two rules that catch most mistakes

1. **Own the surface, rent the model.** A model dependency sits behind our own
   registry and speaks our interface. A *surface* dependency (a chat platform,
   another editor) owns attention, vocabulary and release schedule. Take the
   first freely. Take the second as a ramp, and only where we own the fallback.
2. **Prompt-first, never prompt-only.** Anything doable in an app must be doable
   through the prompt. The reverse is not required.

## Already settled

Two, and both are rooms. Do not re-propose without reading `docs/philosophy.md`
first: chat-platform bridges as *the interface*, and hosting one of our agents
(the Lucidos Agent or a coding agent) inside another editor via a third-party
agent protocol.

Not ruled out, for contrast: notifications that link back in, presence surfaces
in somebody else's shell (a widget, a share-sheet entry) that show state and
hand off in, more clients reaching the same workspace, consuming third-party
capability (MCP servers, provider APIs), and a `lucidos://` scheme (it points at
our own client, so the test does not decide it either way; as *the* deep-link
mechanism it loses on structural grounds in ADR 0048, as an OS handoff it is
open).

**The test is silent on anything pointing at our own client.** Argue those on
cost and mechanics. Do not cite the philosophy to win them.

## Writing a claim into the doc

`docs/philosophy.md` ships verbatim to the public mirror. An aspirational claim
written in the present tense is a lie to contributors, so every sentence in it
must be true of what Lucidos does today, and a claim about mechanism must be
checkable against the code.
