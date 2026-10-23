# 0556: The cache keep-alive covers Lucidos Agent threads: a beat repeats the start of the last request with its generation parameters, on providers whose cache a read refreshes; amends 0470, 0479, 0529 and 0547

- **Status**: Accepted
- **Date**: 2026-10-23
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). Its
  keep-alive covered Claude Code sessions only.
- **Amends**: [0479](0479-keep-alive-follows-the-observed-cache-ttl.md).
  The engine's own requests state their TTL, and a third TTL of 30 minutes
  joins.
- **Amends**: [0529](0529-keep-alive-shows-one-status.md). Lucidos
  Agent threads now show the status too.
- **Amends**: [0547](0547-keep-alive-beats-a-parked-session-by-copy.md).
  It said the Lucidos Agent's parked loops get nothing. They now get beats.

## Context

ADRs 0470 to 0547 keep a Claude Code session's prompt cache warm while it
waits. A Lucidos Agent thread gets nothing: a chat thread, Home, or a trigger
thread. When its sub-thread reports 20 minutes later, the next call re-writes
the context. The cache-warm popover says "Lucidos only keeps Claude Code
sessions warm."

We measured 30 days of Lucidos Agent calls in a busy development workspace. They cost
about $1,950. In 1,675 waits the thread waited on a sub-thread, a keep-warm
event or its own question card. In 749 of them the cache had expired at the
wake. The part of those re-writes a warm cache would have served cost
$314.

The two kinds of wait read the cache differently:

| Wait | What the next request sends | Warm wake reads | Cold wake reads |
|---|---|---|---|
| Turn end: a sub-thread or an event wait | a new turn, with a rebuilt per-turn block | 78% | 18% |
| A card, inside the turn | the last request, extended by the answer | 95% | 17% |

A cold wake still reads a little, because other threads keep the shared tools
and system blocks warm. ADR 0490's stable context block raised the warm turn
read from 53% to 78%.

An earlier plan (`docs/plans/2026-10-15-keep-chat-thread-caches-warm.md`)
chose prefill beats timed by a learned hazard. Only its instrumentation landed:
the call duration, the 1-hour write split and the stable-prefix fingerprint.
Since then ADRs 0489, 0529, 0540 and 0547 built a break-even budget, a status
and one setting. This ADR moves the Lucidos Agent onto that machinery.

## Decision

1. **A Lucidos Agent beat is a prefill call, never a turn.** The engine keeps
   the last request body each thread sent, as its provider built it. A beat
   sends that body again, cut where the next request will read. On the
   Messages wire every parameter of the turn stays, and one text block asks
   for a one-word reply after the last cache marker. The beat reads the
   stream to `message_start` and hangs up. It records one `ContextCaptured`
   with purpose `cache_keep_alive`, and nothing else.
   - **At a turn-end wait** the cut is the end of the stable prefix: the tools,
     the system block, the stable context block and the memory views.
   - **At a card** the beat sends the whole last request. The loop is parked
     inside the turn, so the next request extends it.
2. **Beats go only where a read refreshes a documented TTL and a write costs
   more than a read.**

   | Provider | Cache | Prices against input | Beats |
   |---|---|---|---|
   | Anthropic direct, Claude gateway | 5 min, a read refreshes it | write 1.25x, read 0.025x to 0.1x | yes, behind the first-beat guard |
   | Vertex, Claude | the same | the same | yes, with the turn's parameters |
   | OpenRouter, an `anthropic/` model | the same, with sticky routing | the same | yes, with `max_tokens: 1`: it ignores 0 |
   | OpenAI, GPT-5.6 or later | 30 min minimum, a read extends it | write 1.25x, read 0.05x to 0.1x | yes, with the smallest output cap the API takes |
   | OpenAI before 5.6, Gemini, xAI, local, other OpenRouter models | undocumented TTL, or no write premium | | no |

   Budget thinking (`thinking.type: "enabled"`) beats like any other request,
   because the beat keeps the turn's `max_tokens` and so its budget.

   Probes settled the table (`artifacts/token-cost/lucidos-agent-keep-alive-probes.md`
   in the development workspace). Vertex keys its cache pools on the
   generation parameters (`max_tokens`, `stream`, the effort), not on the
   content after the last marker. A beat with `max_tokens: 0` and no stream
   read the turn's pool in 2 of 12 judged runs. A beat with the turn's
   parameters read it in 12 of 12, also when it hung up at `message_start`.
   OpenRouter accepts 0 but generates a full reply, so it takes 1.

   Direct Anthropic and the Claude gateway could not be probed by hand. They send the same wire as Vertex, so
   they beat behind the first-beat guard below.
3. **The TTL is the one the request asked for**, checked against the usage
   block's 1-hour split. The engine owns these requests, so it knows. A new
   `CacheTtl::ThirtyMinutes` covers OpenAI.
4. **A direct beat fires later than a Claude Code beat.** It has no cold start,
   so its request starts when it fires:

   | TTL | Claude Code window (ADR 0492) | Lucidos Agent window |
   |---|---|---|
   | 5 minutes | 230 to 260 s | 260 to 280 s |
   | 30 minutes | | 1560 to 1620 s |
   | 1 hour | 3300 to 3360 s | |

5. **The budget is ADR 0489's break-even**: the write rate over the read rate,
   in beats. A card gives up at half of it, as in ADR 0547. Under
   `workspace-data`, a wait type is keyed by agent kind as well.
6. **The same waits qualify as for Claude Code.** That is a keep-warm event
   wait, a working sub-thread that will wake the thread, or the thread's own
   question card. The same exceptions hold:
   - a sub-thread parked on the user keeps no parent warm;
   - a `no-wake` sub-thread keeps nobody warm, so Home's filed reports keep
     Home cold;
   - a thread waiting for the user's next message gets no beats;
   - a finished trigger run gets no beats.
7. **Chat and trigger threads follow one rule.** The wait decides, never the
   thread kind. A trigger thread's beat repeats its own request, so its pinned
   model and route hold.
8. **One setting covers both agents.** `cache_keep_alive_mode` keeps its key
   and its three values, and now applies to the Lucidos Agent too. Its section
   moves from Settings > Coding Agents to Settings > Models. Coding Agents
   keeps a one-line pointer.
9. **The status and the popover cover both agents.** A Lucidos Agent thread
   shows the same keep-alive status, transcript card and cache-warm line. The
   popover's `not_claude_code` reason goes. Its replacements:

   | Reason | Popover text after "Nothing keeps it warm while you are away:" |
   |---|---|
   | `nothing_pending` (was `not_parked`) | this thread waits only for your next message. Lucidos keeps a cache warm while a thread waits on a sub-thread, an event or a card. |
   | `uncacheable_provider` | this model's provider has no prompt cache that a ping keeps warm. |
   | `codex` | Codex sessions get no keep-alive: their cache cannot be kept warm this way. |
   | `restarted` (was `session_restarted`) | Lucidos restarted since the last request, so a ping would not match the cache. |
   | `off` | the cache keep-alive is off. |
   | `turned_off` (ADR 0554) | you turned the keep-alive off for this thread at HH:MM. |

Five safety rules hold for every Lucidos Agent beat:

- **A beat never claims the thread.** A turn that starts during a beat runs at
  once, and whichever request lands second reads what the first refreshed.
- **No beat without the remembered request.** After a restart the thread pays
  one re-write.
- **A beat that writes more than 2% of its input ends the run.** The cache no
  longer holds that prefix, so later beats would pay writes too.
- **At most four beats run at once in a workspace.** A refused beat is never
  retried.
- **A provider's first beat is its probe.** A beat can be refused, output
  more tokens than its cap, or write over 2% of its input. Each marks its
  provider unable to prefill until the engine restarts. Its threads then show
  `uncacheable_provider`. The cap is 1 on the Messages wire, which hangs up at
  `message_start` and reads its first token, 1 on OpenRouter and 16 on OpenAI.

## Rationale

- **The engine owns the request, so a beat needs no turn.** Claude Code needed
  a hidden resume or a session copy because Claude Code builds its own
  request. A beat hangs up at `message_start`. It has no output, tool call,
  memory job or read decision, so no turn effect needs a guard.
- **Repeating the last request matches the cache by construction.** The cache
  holds exactly the bytes last sent. A rebuild from the prompt builders would
  drift whenever gates, knowhow or memory views moved, and would need a golden
  test per thread kind. The price is one body per thread in memory, lost on a
  restart. ADR 0547 made the same trade for the appended prompt.
- **A beat shares the turn's cache pool, so it keeps the turn's parameters.**
  A provider may key a pool on `max_tokens`, `stream` or the effort. The
  documented `max_tokens: 0` pre-warm landed in another pool on Vertex, so it
  paid for reads and kept nothing warm. Hanging up at `message_start` stops
  the output, so the model never starts a tool call the engine would run. The
  price is one short text block, about 1% of a large stable read.

  A beat's own usage cannot show a wrong pool, since it reads once that pool
  is warm. The first-beat guard catches a refused or writing beat only. The
  next turn's read judges the pool (week-1 check 4).
- **Break-even instead of a hazard.** ADR 0489's rule needs only two prices,
  and it bounds the worst wait at two re-writes. A hazard rule needs learned
  curves, a shipped default and presence history. The keep-warm wait list
  already drops the next-message waits that sank the flat cut-offs.
- **The simulation pays.** Over the same 30 days the rule saves $278 of the
  $314 and spends $116 on beats, a net $162. That counts 36 event waits that
  never ended, each paying its whole budget.
- **The cold-read share is left out of the budget.** Subtracting it shortens
  the budget to 72% and moves the net by under $1. So ADR 0489's rule stays
  unchanged.
- **Cards are half the saving.** ADR 0470 skipped cards because Claude Code
  blocks inside the hook. The engine's own loop parks, but the engine can still
  send. Card waits net $66 of the $162.
- **The later window keeps ADR 0492's margin.** A direct beat at 260 s starts
  by 270 s, which leaves 30 s before 300 s.
- **One setting, because it is one concept.** It spends the same money for the
  same reason, and a user who turns it off means off. The Lucidos Agent is no
  coding agent, so the setting moves to Models.

## Consequences

- **`ContextPurpose::CacheKeepAlive` gains a second producer.** Token Cost
  already labels the purpose. Claude Code's resume beats still carry the anchor
  link.
- **The provider trait gains a prefill call.** Each provider builds its own cut
  body, or says it cannot. That answer becomes the `uncacheable_provider`
  reason.
- **The candidate query widens past `is_coding_agent`.** `decide()` gains a
  third route, `Prefill`, beside `Resume` and `SessionCopy`.
- **The engine holds one request body per recently active Lucidos Agent
  thread.** It drops the body at the next call, at give-up, or when no wait
  follows within the cache lifetime.
- **The 2026-10-15 plan's hazard rule is superseded.** Its probes of Vertex and
  of effort in the prefix still gate those providers.
- **Expected saving: about $162 per 30 days in that workspace**, 8% of Lucidos Agent
  spend. At the pre-0490 read share it is $125. Packaged workspaces have
  fewer long waits, so less.
- **No 1-hour TTL.** ADR 0470's rejection stands for both agents.

## Alternatives considered

- **A hidden turn, as Claude Code's resume beat.** Rejected. It runs query
  classification, memory recall and the read decision, and it pays for
  output. Each effect needs a guard.
- **Rebuild the prefix from the prompt builders.** Rejected: it drifts, and
  only a repeat of the sent bytes is sure to hit.
- **The hazard rule of the 2026-10-15 plan.** Superseded by break-even, for the
  reasons above.
- **A 1-hour TTL on the stable tier.** Deferred, as in that plan: it raises the
  write price of every call.
- **A second setting for the Lucidos Agent.** Rejected: two switches for one
  idea, and both spend the same money.
- **Keep the setting under Coding Agents.** Rejected: users look for the
  Lucidos Agent's costs under Models.
- **Beat every provider with a cache.** Rejected: an undocumented TTL cannot
  be timed, and with no write premium a miss costs little.
- **`max_tokens: 0` or `1` on the Messages wire.** Rejected: it refreshes a
  different cache pool from the turn's on Vertex. `max_tokens: 1` stays the
  cap where the API ignores 0 (OpenRouter).

Plan, measurements and invariants:
`docs/plans/2026-10-23-lucidos-agent-keep-alive.md`.
