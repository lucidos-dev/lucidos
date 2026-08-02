# Voice control for Lucidos

**Date:** 2026-06-01
**Status:** Note. An architecture discussion written down, not a plan. Nothing is
scheduled, nothing is implemented, and no decision has been made. If this is ever
built it needs an ADR for the decisions and a real plan under `docs/plans/`.
**Topic:** Hands-free voice control of Lucidos using OpenAI's GPT-Realtime-2 speech-to-speech model.

---

## The question

Can Lucidos add voice control built on OpenAI's GPT-Realtime-2 (the May 2026
speech-to-speech model)? What does "voice control" mean, what does it cost, and
how does it fit Lucidos's event-sourced, engine-owns-logic architecture?

## Verdict

Feasible and well-aligned. De-risked by **Clicky / HeyClicky** (Farza Majeed):
the viral build uses GPT-Realtime, proving an S2S "instant assistant" works as a
consumer product. The design below keeps the engine clean and rides Lucidos's
existing primitives (threads, sub-threads, EventBus, MCP, integrations, apps,
coding agents).

## Grounding facts (verified May to Jun 2026)

- **GPT-Realtime-2** (released 2026-05-07): flagship speech-to-speech, GPT-5-class
  reasoning, 128K context, adjustable reasoning effort (default `low`), parallel
  tool calls, interruption recovery, preamble phrases ("let me check…"). Realtime
  API is now GA. Siblings: **GPT-Realtime-Whisper** (streaming STT, $0.017/min)
  and **GPT-Realtime-Translate**.
- **Pricing:** GPT-Realtime-2 audio is token-billed at **$32 / 1M audio-in,
  $64 / 1M audio-out**, cached-in **$0.40 / 1M** (98.75% discount). Audio
  tokenizes at 1 token / 100 ms (user) and 1 token / 50 ms (assistant) → 600
  tokens/min in, 1,200 tokens/min out.
- **Cost trajectory (the important part):** S2S re-sends the whole context every
  turn, so cumulative session cost grows ~O(N²) without mitigation. For Lucidos
  the large system prompt + tool schemas dominate. Real-world voice agents land
  **$0.18 to $0.46/min uncached, $0.05 to $0.10/min with caching**. The two levers that
  bend the curve: **prompt caching** (stable prefix → $0.40/1M) and
  **context truncation** (cap history window). Prices trend down ~quarterly, but
  caching + truncation (engineering available today) beat waiting for price cuts.
- **Cost is explicitly not a design constraint here.** Building for capability /
  the frontier, single-user, not for cheap models.

## Locked decisions

1. **Conversational layer = GPT-Realtime-2 speech-to-speech.** Not an STT/TTS
   bolt-on (Clicky V1's open-source stack: AssemblyAI + Claude + ElevenLabs),
   not the heavy default agent loop. The bolt-on is a proven waypoint; S2S is the
   destination.
2. **A lighter orchestrator on top, never the heavy loop.** "Instant" comes from
   a fast, slim conversational layer + streaming + immediate acknowledgment, not
   from running the full agentic loop per spoken turn (that's the dead-air
   failure mode).
3. **LLM-judged dispatch → sub-threads for heavy work.** The voice session is
   itself a thread; quick turns are answered locally, and when a turn needs real
   work the model's `dispatch` tool spawns a **sub-thread running the full Lucidos
   Agent**. The model speaks a preamble immediately and **narrates async** as the
   sub-thread's EventBus events arrive. (Realtime-2's interruption recovery +
   parallel tools make "keep talking while it works" natural.)
4. **Engine-relayed WebSocket transport.** Browser ↔ engine over a new axum
   WebSocket (`/api/v1/voice`); the engine holds the single GPT-Realtime-2
   connection and mediates everything (function calls, sub-thread spawns,
   injecting results, event-sourcing turns). Because the engine runs **locally**,
   the relay hop is loopback (~0 added latency), so we get full server control +
   statelessness nearly for free. The only unavoidable network hop is
   engine to OpenAI, paid either way. (Rejected: direct WebRTC, because it puts
   session state in the browser and complicates dispatch + event-sourcing. Its
   latency edge
   only matters against a *remote* server, which ours isn't.)
5. **Event model + statelessness.** The voice session is a **thread**. Each
   spoken turn persists as a **transcript**: user turn → `MessageReceived`
   (transcript + `voice` origin marker), model reply → `ResponseGenerated`. New
   thread **`source = 'voice'`** alongside `'chat'`/`'trigger'`. Dispatch reuses
   the existing spawn path (the sub-thread is a normal, already-event-sourced
   Lucidos Agent thread). **Ephemeral (allowed):** raw PCM frames, the OpenAI
   Realtime WebSocket, the browser WebSocket, VAD state. **Restart = dropped
   call, intact conversation:** transcript survives; in-flight dispatched
   sub-threads keep running and auto-resume; on reconnect the voice session
   re-establishes the Realtime connection seeded with the prior transcript and
   re-subscribes to any still-running sub-thread to narrate its now-ready result.
6. **Lean-by-default base (preferred foundation).** Make the agent's default
   context small and fetch more on demand: defer tool schemas (the
   ToolSearch-style pattern this very CC harness uses), lazy-load taxonomy /
   intent registry; `load_knowhow` is already lazy. This is a product-wide
   latency + cost win (chat, triggers, voice) and the multimodal-ready end-state.
   On a lean base the orchestrator's "slim prompt" dissolves into the base; the
   dispatch/async seam stays. Caveat: "fetch when needed" can add a mid-turn
   stall (masked by the preamble/narration habit), and a smaller prompt can
   cache *worse* (smaller stable prefix). Blast radius is large (core prompt/tool
   assembly), so it's its own workstream.
7. **Action scope = operate Lucidos's world (API-level).** Voice acts through
   Lucidos's existing action surface: **email** (`email_accounts`,
   `/api/v1/email/send`), **MCP servers** (`mcp_servers`), **integrations**
   (OAuth, Notion/Gmail/Calendar-style), **apps** (`create_app`, run/build), and
   **coding/research sub-threads**. This is most of Clicky's actual value
   (Clicky's own YC blurb is integration + codegen). NOT pixel-level puppeting of
   arbitrary on-screen apps from the engine.

## Computer use, as knowhow rather than engine core

Pixel-level computer use (driving external apps, flying the cursor) is **out of
the engine core.** Lucidos is web/PWA + Tauri and has no native automation
layer, and baking one in is effectively a different product (a general
computer-use agent) with a major security model.

But it doesn't have to be blocked. Framed as **knowhow**, it rides Lucidos's
existing extensibility model:

- **Execution** lives in a pluggable **MCP server** outside the engine:
  browser automation (Playwright-style), desktop automation, or an OpenAI
  computer-use sandbox. This is the API-level handle on computer use.
- A **knowhow** file documents *when and how* to use that MCP tool.
- An optional **script** wraps any repeatable procedure.
- The voice agent **dispatches** to a sub-thread that loads the knowhow and
  drives the MCP tool.

Net: the engine stays clean and web-first; computer use is a capability the user
can plug in (MCP integration + knowhow + script), reached through the same
dispatch path as any other heavy work. No special engine feature required.

## Defaults on minor opens (revisit at build time)

- **Context source:** **workspace-only** (threads / events / apps / integrations).
  Screen-awareness was only needed for the pixel-level path, which is out.
- **Activation:** **push-to-talk** (hotkey/button), not wake-word. Simpler, no
  always-listening privacy surface. Surfaces: desktop / Tauri first; PWA where
  mic permissions allow.
- **Audio storage:** **transcript-only**; raw audio discarded after playback
  (cheapest, statelessness-clean, no audio-privacy surface). Persisting audio as
  content-addressed blobs is a possible later add for replay.

## Suggested build order

- **Phase 0, lean base.** Defer tool schemas, lazy taxonomy/intent (knowhow
  already lazy). Product-wide latency/cost win; voice quality depends on it.
- **Phase 1, voice transport.** New axum WebSocket `/api/v1/voice`; browser mic
  capture (reuse the existing `AudioContext` unlock shim) + audio playback;
  engine ↔ GPT-Realtime-2 relay; push-to-talk. Voice session = thread;
  transcripts event-sourced.
- **Phase 2, orchestrator + instant actions.** Slim voice prompt (or just the
  lean base); curated instant-action toolset (open app, navigate, query/emit
  events, send email, apply a named pending change, hit MCP tools/integrations).
- **Phase 3, dispatch + async narration.** `dispatch` tool → spawn sub-thread
  (full Lucidos Agent); subscribe to its EventBus; inject voice-friendly
  summaries; narrate async; restart resilience (re-dial + re-subscribe).
- **Later / optional.** Computer-use knowhow + MCP server; persisted audio blobs;
  GPT-Realtime-Translate for live translation; wake-word activation.

## Surfaces touched (docs must land with code)

Per `.claude/rules/system-knowhow.md`, building this touches documented surfaces
that must be updated in the same change:

- `system-knowhow/thread-events.md`: new thread `source = 'voice'` + the
  `MessageReceived`/`ResponseGenerated` voice origin marker; any new variant gets
  a row + the streaming-blocklist flag.
- `system-knowhow/glossary.md`: a **voice session** entry (proposed below).
- `system-knowhow/js-sdk.md`: if a `lucidos.voice.*` SDK surface is added.
- `.claude/rules/db.md`: if any new `SystemEvent` variant is introduced.
- New credentials: OpenAI Realtime API key (the engine already has a
  credentials/proxy pipeline to reuse). GPT-Realtime-2 emits voice natively, so
  no separate TTS provider is required.

## Proposed glossary entry (add to `system-knowhow/glossary.md` when built)

> **voice session**: a *thread* whose conversational turns are driven by a
> realtime speech-to-speech model (GPT-Realtime-2) over an engine-relayed
> WebSocket, rather than by typed messages through the agentic loop. It answers
> quick turns itself with a slim toolset and **dispatches** heavy work to
> sub-threads running the full Lucidos Agent, narrating their progress as
> EventBus events arrive. Turns persist as transcripts (`source = 'voice'`); the
> audio stream and the live connection are ephemeral.

## Open questions for build time

- Exact slim toolset for instant actions, and the dispatch decision boundary.
- How sub-thread results are summarized into voice-friendly narration (and by
  whom: the sub-thread, a summarizer, or the voice model reading a trimmed
  EventBus feed).
- Caching strategy for the stable prefix + context-truncation window.
- Mobile/PWA mic + WebSocket viability vs. desktop/Tauri-first.

## Sources

- OpenAI: [Introducing gpt-realtime](https://openai.com/index/introducing-gpt-realtime/),
  [Advancing voice intelligence](https://openai.com/index/advancing-voice-intelligence-with-new-models-in-the-api/),
  [API pricing](https://openai.com/api/pricing/),
  [Realtime cost guide](https://developers.openai.com/api/docs/guides/realtime-costs),
  [gpt-realtime-2 model](https://developers.openai.com/api/docs/models/gpt-realtime-2)
- [GPT-Realtime-2 release (MarkTechPost)](https://www.marktechpost.com/2026/05/08/openai-releases-three-realtime-audio-models-gpt-realtime-2-gpt-realtime-translate-and-gpt-realtime-whisper-in-the-realtime-api/),
  [DataCamp deep-dive](https://www.datacamp.com/blog/gpt-realtime-2),
  [CallSphere cost math](https://callsphere.ai/blog/vw2c-openai-realtime-cost-per-minute-math-2026)
- Clicky / HeyClicky: [clicky.so](https://clicky.so/),
  [GitHub (open-source V1)](https://github.com/farzaa/clicky),
  [how Clicky works (Isaac Flath)](https://isaacflath.com/writing/how-clicky-works),
  [YC profile](https://www.ycombinator.com/companies/heyclicky)
