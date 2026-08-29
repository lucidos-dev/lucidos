# Voice control for Lucidos

**Status:** Superseded. This note was an architecture discussion, written before
any decision was made. The decisions were made later, in a design thread, and
most of what this note proposed lost.

**Read instead:**

- `docs/plans/2026-08-28-voice-joins-a-thread-as-a-participant.md`, the plan.
- ADR 0148, voice is a mode of a thread.
- ADR 0149, the rented tool-less talker beside the agent we own.
- ADR 0150, an agent-authored event names its author.
- ADR 0151, the gateway carries a WebSocket upgrade.

Two sections of this note survive, and they are kept below because nothing else
records them. Everything not in those two sections is superseded.

## What this note got wrong

Listed so nobody resurrects it from here. Each row's reasoning is in the ADR.

| This note proposed | We decided | Where |
|---|---|---|
| A new thread `source = 'voice'` | Voice is a mode of a *chat thread* | ADR 0148 |
| A slim voice agent with a curated toolset | The talker holds no tools at all | ADR 0149 |
| A `dispatch` tool spawning sub-threads | Both models sit on one thread | plan, non-goals |
| The voice model narrating async progress | One voice, first person, one entity | ADR 0149 |
| Rejecting WebRTC on latency grounds | Rejected on credential grounds | ADR 0151 |

The note also proposed a *voice session* glossary entry. It is wrong on both
`source` and dispatch, so the plan writes a fresh one in the phase that builds a
session.

## What still holds: the cost shape

A speech-to-speech model re-sends its whole context every turn. So a session's
cumulative cost grows superlinearly with its length, and the system prompt plus
tool schemas dominate. Two levers bend that curve, and both are engineering
available today rather than a price cut to wait for:

- **Prompt caching**, which needs a stable prefix. A cached prefix is roughly
  two orders of magnitude cheaper than fresh input.
- **Context truncation**, which caps the history window.

The plan's append-only invariant comes straight from this. Deleting one history
item mid-session was measured to triple full-price input for that turn, because
it invalidates the cached prefix behind it. So the rule is to append, and to
trim rarely in large steps.

Cost is not a design constraint for Lucidos here. This is a single-user system
built for capability. The shape matters anyway, because a session that gets
slower and dearer the longer you talk is a product problem, not a bill.

Live per-token prices belong on the provider's pricing page, not in this file.

## What still holds: computer use as knowhow

Pixel-level computer use stays out of the engine core. Lucidos is a web and PWA
client plus a Tauri shell, with no native automation layer. Building one is a
different product with a much larger security model.

It is not blocked, though. Framed as *knowhow* it rides the existing
extensibility model:

- Execution lives in a pluggable **MCP server** outside the engine: browser
  automation, desktop automation, or a hosted computer-use sandbox.
- A **knowhow** file records when and how to use that tool.
- A **script** wraps any repeatable procedure.
- A sub-thread loads the knowhow and drives the tool.

The engine stays web-first, and computer use becomes a capability the user plugs
in. No engine feature is required.

This is independent of voice and outlived the note that proposed it.

## Sources

The original note's grounding links, kept for provenance:

- OpenAI: [Introducing gpt-realtime](https://openai.com/index/introducing-gpt-realtime/),
  [API pricing](https://openai.com/api/pricing/),
  [Realtime cost guide](https://developers.openai.com/api/docs/guides/realtime-costs)
- [Introducing GPT-Live](https://openai.com/index/introducing-gpt-live/),
  [how the realtime system was built](https://openai.com/index/continuous-voice-interaction-with-gpt-live/)
- Clicky: [clicky.so](https://clicky.so/),
  [how Clicky works](https://isaacflath.com/writing/how-clicky-works)
