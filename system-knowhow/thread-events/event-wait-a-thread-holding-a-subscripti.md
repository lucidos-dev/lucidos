---
name: ThreadEvent Reference, Event wait (a thread holding a subscription)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Event wait (a thread holding a subscription)

The lifecycle of one `await_event` call. All four are persisted, and all four are
**triggerable but never awaitable** (see § "Triggerable is not the same question
as awaitable"). There is no `thread_event_waits` table: `EventWaitStarted` *is*
the wait, and the dispatcher rebuilds its live set from these rows at boot.

| Event | When it fires | Volume | Persisted | Triggerable | Awaitable |
|---|---|---|---|---|---|
| `EventWaitStarted` | A thread registered a subscription. Emitted between the `ToolCalled` and its `ToolResult`, so the pair closes normally and the turn carries on. Carries `wait_id`, `tool_use_id`, `on: EventSubscription[]` (same shape as a trigger's `on:`), `reason` (the model's own words, shown to the user), `armed_at`, `expires_at`, and `watermark`. The watermark is the event `sequence` at registration, where the catch-up scan starts. `armed_at` is recorded, not derived from `expires_at`, so the age `list_event_waits` reports never drifts. Rows from before 2026-08-07 lack it and fall back to the row's own `created`. Writes NO status. | per-action (rare) | yes | yes | **no** |
| `EventWaitTimingUpdated` | A background task entered or left the build-slot queue. Updates the existing wait's `reason` and `expires_at` by `wait_id`. | per-action (rare) | yes | yes | **no** |
| `EventWaitDelivered` | A matching event resolved the wait. Carries `wait_id`, the matched `event_id` / `event_type` / `payload` (self-contained, so replay never dangles), and `matched_index` (which `on:` entry fired). | per-action (rare) | yes | yes | **no** |
| `EventWaitExpired` | The wait passed `expires_at`. **Re-opens the thread** with an explanatory message, because a silently dropped wait is a permanently stalled thread. Carries `wait_id`. | per-action (rare) | yes | yes | **no** |
| `EventWaitCanceled` | The subscription was stopped before its own resolution. `cause` is one of `user_stop` (the **Stop waiting** button), `agent_stand_down` (the agent retired one of its own, or stopped the background task it watched, see `BackgroundBashCompleted`), `thread_archived`, `thread_discarded`. Neither an ordinary user message nor a thread-level **Stop** touches a subscription. `thread_canceled` is a RETIRED cause, still read so pre-2026-08-07 rows replay, never emitted. Also carries `on` and `reason`, a copy of what was stopped. So the transcript entry stands alone on replay, like a delivery, even when its `EventWaitStarted` is outside the loaded window. Both are absent on pre-2026-08-07 rows. Because a cancel re-opens nothing, it is when `todo_consumer` settles a parked *todo list*, unless a turn still owns it (see `TodoListWritten`). | per-action (rare) | yes | yes | **no** |

### A subscription does not hold the turn

`await_event` returns immediately, like any other tool. The turn ends with an
ordinary terminator, and the thread is then plain **`idle`** while it watches:
no queue slot and no blocking state. Archive stays offered, and archiving
cancels the subscription. The per-thread **waiting indicator** shows a live
subscription, not the thread status.

**One thing does wait with you: a change you already proposed.** The thread
reads *Waiting* rather than *Changes to review*, and Apply and Discard are
withheld until nothing is left to wake it. You may commit again after the
delivery, so applying now would merge a branch still in progress. The user's
way out is **Stop waiting**, which ends the subscription and returns both
buttons. A turn that parks with a change pending tells the user "not yet".

**A sub-thread you are waiting on does not hold your change.** It works in its
own worktree and never writes your branch, and your committed work is whole at
each turn's end. So a thread idle apart from running children reads *Changes
to review*, and the user can Apply or Discard. Work you commit after a child's
completion wakes you comes back as a new change. An apply clears your branch's
plan and harden markers, so a Lucidos-source thread plans and hardens that new
round again. So never hand your own change to a "carrier" child.

#### It already happened: the registration result may hand you the answer

A subscription watches **forward only**, so it never fires for something
already past. If the thing might be in the past, check state first. A wait for
an event that already happened just idles until the timeout.

You need not worry about the **race between that check and the call**. If a
match landed in the few minutes before it, registration names it in the
`await_event` result, with its payload and its age.

**That is a report, not a delivery.** The subscription will never fire for
anything the result names. A turn that ends without acting on it leaves the
thing unhandled. Only you can tell an event you missed from one you handled
earlier in the same turn. Act on it now, or say that you already did.

Only an event an earlier wait handed you (an `EventWaitDelivered`) suppresses
that report. So a re-arm right after a **delivery** is never told about the
event it was just handed.

That covers a delivery, not every re-entry. A *child-completion* callback also
re-opens a thread, and the fan-in writes no `EventWaitDelivered`. A re-arm on
`ChildThreadCompleted` in that turn can name the very callback that re-opened
you; recognise it by `child_thread_id` and age, and carry on. Better, never
subscribe to your own child: the `ChildThreadCompleted` section explains why.

#### Which resolutions leave you still watching, and which do not

| Wake | Subscription after it | What to do |
|---|---|---|
| **Delivery** (`EventWaitDelivered`) | **Spent.** The first match resolves the wait and consumes it. Any *other* live wait on the thread is untouched. | To catch the next one, call `await_event` again *before the turn ends*. Saying you will re-subscribe is not re-subscribing: a turn that ends with no new call leaves nothing watching. |
| **Expiry** (`EventWaitExpired`) | Gone. | Report what you were waiting for, rather than subscribing again to the same thing. |
| **Stopped** (`EventWaitCanceled`) | Gone, because somebody stopped it: the **Stop waiting** button, an archive or discard, or you standing it down. There is no re-entry, so the thread is left as it was. | Report back. Do not re-register unless they ask. |

Delivery is the one that bites. It alone consumes the subscription *and* hands
you a payload, so it reads like the wait is still running. A standing in-thread
watch is one subscription per event, bounded by the recent-subscription cap in
§ "Limits": past it, `await_event` is refused and you report back. Never promise
the user "forever" in a thread; that is a trigger's job.

A **user message** and a thread-level **Stop** are absent from the table on
purpose. Neither resolves anything, so every subscription survives with its
deadline intact. Stop ends the running turn, which no subscription was holding.

**No `EventWait*` event writes a status**, by rule. Registration happens
mid-turn, so the turn's own terminator decides. A resolution lands on an idle
thread, whose own `PromptInjected` sets `running`, or on one running
unrelated work. A status write there would misreport it as revived.

Waits were once **attached** to an unpaired `tool_use`. That shape is gone: see
`docs/plans/2026-08-06-every-event-wait-is-detached.md`.

### The re-entry anchor

Every delivery and every expiry is followed by exactly one more event that
carries the payload as prose: a **`PromptInjected`**. It starts a new
exchange, the honest shape for something that may arrive hours later. A
child-thread completion re-opens its parent the same way.

**It says the event arrived, never that you were asleep.** Registration does not
hold your turn, so a match can land while this thread is still working. The
engine then folds it into the running turn and says it arrived "while you
were working". The transcript card marks it `arrived` for the same reason: a
delivery does not know which of the two lanes it took.

On a *delivery* it also carries `delivered_event_id`, the id of the
`EventWaitDelivered` above it. The prose is the model's prompt and cannot be
trimmed, but rendered verbatim it is a screen of JSON. So the transcript reads
`event_type` and `payload` from that row and names the event with its payload
folded away. An *expiry* leaves the field unset, having no payload.

Both carry `origin: { kind: "engine", reason: { kind: "event_wait", outcome,
watched, wait_reason } }`, with `mode: "agent"`. `outcome` is `delivered` or
`expired`. `watched` names each event type the wait watched, once. `wait_reason`
is the reason the agent gave when it subscribed. The transcript reads these to
say who wrote the prompt and why. Older rows carry no `origin`.

On restart this matters: a resolution followed *only* by its anchor is one whose
turn never ran. That is how the engine re-drives a re-entry lost to a crash.

### Both agents, one registration

The chat agent registers through the `await_event` LLM tool. A **coding agent**
uses `lucidos await-event` (see `lucidos-cli`), which POSTs
`/api/v1/threads/<id>/event-waits` into the same code. So the caps, the
subscribability gate and the refusal wording are one implementation. The
delivery routes down the coding-agent lane (into a live session, or a fresh
resume when there is none), exactly as a child completion does.

### Discover cards from sub-threads

Subscribe to `SubThreadCardOpened` with `{"ancestor_thread_id":"<this-thread-id>"}`.
The engine offers a copy only to its addressed thread, even if the condition
is omitted. It covers sub-threads at any depth, including those spawned after
arming. It works with any wake mode. Triggers cannot subscribe to this event.

The matchable payload has five fields. `ancestor_thread_id` names the
recipient. `child_thread_id` is the first hop toward the source.
`source_thread_id` and `source_event_id` identify the original opening.
`card_kind` is `question` or `permission-request`.
Read and answer the card in the source thread using the usual authority.

Arming reports matching cards still open in the current tree, even if they
are old. The report leaves the wait armed for a new opening. If it includes
`next_after`, pass it as `after` along with `wait_id` to `list_event_waits`.
Re-arming can report a card again while it is still open. After a sub-thread
moves to top level, reports and new openings use the new tree. A wait can
still deliver an opening committed for it before the move.

### Limits

Three, all refused at registration with an error the agent reads in the same
turn:

- `timeout_secs` is **required** and capped at **24 hours**. There is no
  unbounded wait. Anything longer is a trigger.
- A thread may hold **25 live waits** at once, and may not register the same
  `on:` list twice (one event would then deliver twice). The limit is on
  outstanding re-entries, not on what you watch. One wait's `on:` list is
  uncapped, so a dozen things in one subscription (any entry delivers) costs one
  of the 25.
- A thread may start **20 counted waits within an hour** with no message or
  question-card answer from the user in between. **A wait another thread's event
  ended does not count**, so waiting on other threads one at a time never
  reaches it. Everything else counts:
  - a wait ended by this thread's own event;
  - a wait ended by an event with no known source (`events(action="emit")` writes those);
  - a wait that timed out, and one not yet ended.

  That bounds a thread that re-opens itself and a model stuck re-arming. An
  agent- or engine-authored message does not reset the count. Engine-armed
  waits for this thread's own background tasks count too.

### The cache keep-alive status

While a Claude Code thread waits, the *cache keep-alive* keeps its prompt cache
warm. Each ping turn leaves no trace (§ Engine origins), but one status event
says what the keep-alive does as a whole (ADR 0529).

| Event | When it fires | Volume | Persisted | Triggerable | Awaitable |
|---|---|---|---|---|---|
| `CacheKeepAliveStatusChanged` | The keep-alive status of a waiting thread changed. The keep-alive tick is its one writer, and it emits only on a change: a new ping, a cost update, a stop, an end or a cleared status. A wait parked on a question or permission card writes one too. Carries `status`, absent once the thread has nothing to keep warm. Otherwise `status.state` is one of five. `warming`: `since`, `beats`, `budget_beats`, `beat_every_secs`, `cost_usd`, `context_tokens`, optional `next_beat_at`, `give_up_at`, optional `odds` (`samples`, `slots`, `after_cap`: how past waits of the same agent and type ended against the cap), optional `expected_wake` (`at` and `source`: the expected wake the plan follows). `budget_beats` counts the pings the plan allows this run, from `since` to `give_up_at`. A learned cap reports its own ping count instead. `stopped` (a run that pinged, then reached the end of its plan): `at`, `beats`, `cost_usd`, `context_tokens`. `ended` (the wait ended while kept warm): `since`, `at`, `beats`, `cost_usd`, optional `saved_usd`, optional `context_tokens`. `turned_off`: `at`, `beats`, `cost_usd`, `context_tokens`. These four also carry optional `ping_usd` and `reread_usd`, today's list price of one ping and of rereading the context. The three closed states also carry the optional `beat_every_secs` and `odds` of the run's cap, so the math reads the same in every state. These four also carry an optional `cap_source`: what set the give-up time, tagged by `kind`. It is `half_reread` (a parked card's fallback), `reread_cap`, `timeout` (the wait's own timeout) or `estimate` (with `by`: `agent` or `decision`). Or it is `learned`, a parked card's cap learned from the cards asked near its local hour. That carries `pings`, `samples`, and `from_hour` up to `to_hour`, which may wrap past midnight. Its `utc_offset_mins` is the `timezone` preference zone's offset at the ask, so the card shows those hours on the user's clock. A status stored before the field has none. `not_kept`: `reason`, one of `no_wake_children`, `children_awaiting_user`, `uncacheable_provider` (the last request has no prefill pair or backend), `provider_paused` (a refused or talkative ping paused this provider until restart), `prefill_paused` (historical: old events carry it, and the engine no longer emits it), `pings_paused` (the provider's recent pings kept missing, so they pause until the status's `until`), `cache_expired` (the cache went cold before a ping reached it), `restarted` (Lucidos holds no record of that request: it restarted since, or the Claude Code session ended), `past_give_up_time` (the wait passed its give-up time, before the first ping was due or after the engine released the request), `wake_past_reread_cap` (the expected wake lies past what pings may cost, so none go out; these two also carry the `cap_source` that set the limit), `model_changed` (the beat would use another model or effort), `beat_missed` (a ping did not hold the context; only a real request re-arms it) or `unknown_ttl` (no reply said how long the cache lives, so no ping can be timed). A trigger thread no person has written in gets no ping and records no status. Metadata: it opens no turn and moves no status, section or recency. Projects into `thread_summaries.cache_keep_alive_status`. The transcript folds a run's statuses into one card, opened by the first warming status, before any ping (ADRs 0540, 0563 and 0572). A run that ends before its first ping closes as `ended` with zero `beats` and no `saved_usd`. A run a ping ended counts that ping in its `beats` and `cost_usd`. | per-action (rare) | yes | yes | yes |
| `CacheKeepAliveTurnedOff` | The user turned the cache keep-alive off for this one thread ("Turn off for this thread", or the `threads` tool's `cache_keep_alive` action), ADR 0554. No payload; `actor` names who. Projects into `thread_summaries.cache_keep_alive_off_at`. It holds until the thread's next real request, so a keep-alive ping never ends it. Metadata, and the transcript draws nothing. | per-action (rare) | yes | yes | yes |
| `CacheKeepAliveTurnedOn` | The user turned the cache keep-alive back on for this thread. No payload; `actor` names who. Clears `thread_summaries.cache_keep_alive_off_at`. Metadata, and the transcript draws nothing. | per-action (rare) | yes | yes | yes |
| `WakeExpected` | The thread's *expected wake*: when it will next wake, from the earliest of its event waits and the children that report to it (ADR 0603). Carries `at`, `source` (`agent` for `expect_wake` or `lucidos expect-wake`, `decision` for the background decision), `reason`, and an optional `distribution` (a list of `by` and `probability`, the odds per time bucket). `actor` names the agent thread for an agent's statement. The newest one stands. Metadata: it projects nothing, and the transcript draws nothing. | per-turn | yes | yes | yes |

### The expected wake

The keep-alive plans its pings from the thread's newest `WakeExpected`. The
agent states it before ending a turn that leaves a wait or a child pending:
the chat agent with the `expect_wake` tool (`minutes`, `reason`), a coding agent
with `lucidos expect-wake <duration> --reason <R>`. The span runs from one
second to 24 hours.

With the keep-alive on, an `await_event` registration on a keep-warm event asks
for the statement. It adds how long past waits of the same agent and type
lasted in this workspace over 30 days: how many, the median and the 80th
percentile. Those figures are an input to the estimate, never a rule.

A turn that leaves something pending and states nothing gets the *expected wake
decision*: one background call at the turn end, recorded as `WakeExpected` with
`source: decision` and no actor. Its chat model picks one time bucket, so `at`
is that bucket's middle and `distribution` is empty. A System One model fills
`distribution` with a probability per bucket, and `at` is its median. No `at`
lands past the earliest event wait's timeout. A failed call records nothing.

### The ping plan

The *reread cap* is the pings that together cost one reread of the context. It
prices every plan:

- **A plan looks forward** from the estimate. A point estimate pings to 1.25
  times the expected wait, and at least one ping past it. A distribution pings to
  the bucket end with the best expected saving.
- **A plan runs only when its pings cost at most 80% of one reread.** Otherwise
  no ping goes out, and the status is `not_kept` with `wake_past_reread_cap`.
- **Two caps hold whatever the estimate says.** No ping lands past an event
  wait's timeout, and no run pings past the reread cap from the wait start.
- **A plan that runs out with no wake asks the decision once more.** The new
  call sees the earlier estimates that passed. A new estimate plans forward, and
  without one the run stops.
- **An estimate covers a wait** when the turn that stated it ended after the
  wait began. With none, the bare reread cap applies from the wait start.

A wait parked on a question or permission card has no estimate, since no model
can say when a person answers. Its cap is *learned* instead (ADR 0606):

- **From the cards asked near its hour.** It takes this workspace's cards of the
  same agent and card type, asked within two hours of the ask's local hour.
  The hour is in the `timezone` preference, UTC when unset. It picks the ping
  count with the least summed cost over them.
- **A card's wait ends when the cache is next read**: at the first real request
  after the card closes. An answer resumes the turn at once, and a dismissal
  waits for the next message. An allowed permission card ends at the allow,
  since its tool runs first. With no real request, the card stays out of
  history for a day, then counts as never answered.
- **Never past the reread cap.** Below 30 cards across 7 days in that window,
  the card keeps half the reread cap.
- **Frozen per wait.** The cap and its odds resolve at the wait's first status,
  and later ticks read them back. The hourly history rebuild never moves them.
- **A cap of c pings sends c pings.** The run counts its pings and stops after
  the c-th. Its give-up time is the last refresh, plus the beats left, plus
  the beat window's width. With every ping out, it is the last refresh.
- **The odds count the same window.** The math on the card explains the cap
  with the cards that set it.

