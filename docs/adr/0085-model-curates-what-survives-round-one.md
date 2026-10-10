# 0085: The model curates what survives round 1, and the harness drops nothing that has no way back

- **Status**: Superseded by
  [ADR 0109](0109-model-writes-notes-and-sees-its-own-context.md)
- **Date**: 2026-08-17

The preference survives and so does the experiment. What 0109 replaces is the
mechanism. The ledger becomes a context panel carrying size, age and percent of
budget. The model gains a scratchpad to write into, and `keep_in_context`
becomes a pin the trimmer honours. `dismiss_from_context`, the assembled body
region and the arrival window all go. Read 0109 for what the mode does today.

## Context

The harness decides what the agent remembers, and it decides by position.
`engine/context.rs` keeps the last 4 messages verbatim and compacts older
assistant messages to 1,500 chars. It summarises past 15 messages, and caps a
turn's tool summary at 2,000 bytes. `store/messages/resume.rs` rebuilds the last
3 tool pairs verbatim and pins `load_knowhow` results past the window. None of
those numbers is a judgment about the thread. They are a judgment about recency.

The message array is `RESUME_VERBATIM_TOOL_TAIL` pairs first, then one bundled
user message (`chat/process/run.rs:1402`). That head is the 3 most recent tool
calls of the turn that just ended, so it rotates wholesale at a boundary. It was
measured changing at 94.3% of 1,115 boundaries. The messages tier therefore
mutates at index 0, and 100% of it is rewritten. There is no middle mutation and
no stable prefix inside it to preserve.

A 30-day measurement priced it, in
`data/artifacts/context-economics-investigation.md` over 19,254
`ContextCaptured` rows. A turn boundary writes 67,589 tokens and costs $0.4415,
at 47 a day, so 25.0% of the Opus bill. The larger line item is within-turn
cached reads, at 43.5%. Three causes split the boundary write. The clock takes
about a third, which ADR 0084 fixed. The messages[0] rotation takes about a
half, and ~9,200 tokens of unmatched tools prefix take a seventh.

What 0084 left standing is this decision's target. Its own words, rejecting
per-line history timestamps: "history is a stringified blob inside the user
message, rebuilt per turn ... that blob is rewritten at every boundary
regardless."

Lucidos holds an asymmetry that changes what is safe here. The event store is
lossless, so trimming removes something from the prompt and not from existence.
Claude Code cannot do this, because its transcript is its only memory. That is
why its answer is compaction, and why ours does not have to be.

## Decision

An **experimental context mode**, a workspace preference, off by default and
available for any model. When it is on:

1. **One mutable block sits at the tail.** `todo_write` is extended to carry
   notes beside its items. Replace-whole-list semantics are unchanged, so the
   model rewrites and shrinks both freely. Notes render in the UI beside the todo
   items for the duration of the experiment.
2. **Exactly two sections stop being sent, and the unit is the round.** Round 1
   of the thread carries everything. From round 2 on, memory recall and the
   conversation history are gone. Every other section stays exactly where it is
   today. A round is where a cached prefix is re-read, so this is the saving.
3. **Tool pairs are the one thing the turn boundary drops.** They accumulate
   within a turn as today and go at its end, replaced by whatever the model
   noted.
4. **Memory recall runs on the first call only.** The thread's first call recalls
   as it does today and the model notes what matters. From then on the agent
   reaches memory through a memory search it decides to make.
5. **Nothing is dropped that has no way back.** Every dropped section names its
   recovery tool. A section with no tool either stays, or gains one before it can
   be dropped.
6. **Active context stays.** Which app, file or URL the user is looking at is a
   fact about this moment, not memory, and no tool can re-fetch it.
7. **The todo list stays the stated objective and never becomes the loop
   condition.** ADR 0071 and `MAX_TODO_WAKE_NUDGE = 1` are untouched.
8. **The recovery instruction lives in the cached prefix.** The standing
   re-read rule goes in the system prompt, and the queryable event types go in
   the `events` tool schema. Neither belongs in the volatile block, because
   neither ever changes.
9. **A live tool result states its own event id.** Nothing is retained by this:
   the pair still vanishes at the boundary. The id appears in the result text so
   the model can note the address of something it is about to lose. Today the
   live turn shows only the provider's `tool_use_id`, and the `evt-<id>` form
   exists on the resume path this mode deletes.
10. **An image is noted by description first, and by handle second.** A text
    result is large and its pointer is tiny, so the pointer wins. An image is a
    fixed 1,600 tokens and the useful part is usually one sentence, so the
    description wins. The handle is the fallback for when the description
    proves too thin.
11. **The image handle must stop being positional.** `view_image` takes
    `thread:N`, numbered as shown in the conversation history, and this mode
    deletes that enumeration. Either a thread image gets a stable id like a tool
    result, or the model persists it with `save_thread_image` and notes the
    artifact path.
12. **Chat threads and trigger threads, not coding-agent threads.** Both run
    `chat::process`. A coding-agent thread is out by construction: Claude Code
    and Codex build their own context and keep their own todo list.
13. **Every re-entry carries the payload again.** An answer-driven resume, a
    manual Continue and an event-wait delivery each count as a round 1. The
    prefix is cold on any of them, so the cost is a write rather than a lost
    read. A thread waking hours later gets its history back and a fresh recall,
    rather than working from its notes alone.
14. **Nothing forces a note, and nothing checks for one.** The payload goes at
    round 2 whether or not the model wrote anything. No earned drop, no nudge,
    no floor. A model that cannot curate produces worse answers, which is the
    honest signal.

The flag ships behind the switch on those terms, and the cost side is measurable
from the event store. `cache_creation` per turn boundary comes from
`ContextCaptured`, and the rate at which the model dereferences a note through a
recovery tool comes from `ToolCalled`. Both read alongside rounds per thread and
`TodoListWritten` events per thread, which is what break-even needs.

**Graduating to the default is a separate bar, and no number here clears it.**
That takes a task eval: two workspaces seeded identically, running the same
sequence of threads. One arm lean, one not, scored on whether the task got done.
It is its own piece of work with its own record, and that record is ADR 0087.

Two constraints on that eval are worth stating now. A workspace built from
scratch has no memory recall and no conversation history. So the first task in
each arm sends identical prompts and measures nothing, and the sequence has to
accumulate state before it tests anything. An agent is also not deterministic,
so the unit is tasks times repeats times two arms.

## What the notes carry

One rule: note what you cannot cheaply re-derive, and note the address of
everything you can.

Two categories are genuinely lossy, so the guidance has to name them:

- **Dead ends.** Manus found that failures left in context stop the model
  repeating them. This mode drops the failures. Unnoted, the agent retries what
  already failed, and that is the most predictable regression here.
- **Constraints the user stated in conversation.** "Do not touch the frontend",
  "call it X not Y". Each is said once, in a message that is then gone.

Four more, in descending value:

- decisions taken, and why
- pointers: event ids, file paths, URLs, thread ids
- state created outside the prompt, such as a background task, a sub-thread or
  an event-wait subscription
- what the thread is blocked on

What not to note: anything one cheap call brings back, where the address is the
note. Anything the harness still sends every round is pure duplication.

## Rationale

**Position and volume are both causes, and this removes both.** Anthropic caches
by prefix in the order tools, system, messages, hashing the exact bytes up to
each breakpoint. The rotating resume tool pairs sit at messages[0], so they
invalidate the entire messages tier at every boundary. That is about half the
boundary write, and dropping them removes the mutation outright. Dropping memory
recall and the conversation history from round 2 is the volume half, and it
lands on the larger line item.

**The prior art is two retreats, and neither is the retreat it looks like.** Manus
shipped the rewritten `todo.md` in July 2025, learned on Sonnet 3.5 and 3.7. They
retired it because roughly a third of all actions went to updating the list.
Letta shipped self-rewriting memory blocks, whose lineage is the MemGPT paper of
October 2023 on GPT-4. They moved the editing to asynchronous sleep-time agents
in April 2025.

Neither team concluded that model-curated memory is wrong. Both concluded that
doing it synchronously inside the acting loop is expensive. That is a claim about
cost and timing, and it is the kind that decays as models improve.

**Anthropic has not dismissed it either.** Their guidance recommends structured
note-taking and states the objective as finding the smallest set of high-signal
tokens. They keep compaction alongside it because Claude Code has nowhere to page
back from. Their platform answer for tool output is result clearing, which drops
a body and keeps the record of the call. The industry has converged on pointers.
What nobody has done is hand the pen to the model, and that narrower claim is
what this tests.

**The lossless store converts a prediction problem into a noticing problem.** The
standing objection to write-time curation is that round 3 must guess what round
30 needs. A better model has a better prior, not clairvoyance. That objection
does not decay with capability. It is dissolved instead by recovery: a wrong
guess costs a tool call. The question becomes whether the model notices it lost
something, which is metacognition, which does improve.

**Recoverability is the floor, not retention.** A verbatim tail the harness pins
is the harness deciding again, in smaller print. Guaranteeing that every pointer
resolves is the same protection with none of the per-turn cost. Its failure mode
is a wasted tool call rather than silent amnesia.

**Off by default and available for any model, rather than an allow-list.**
`.claude/rules/temporary-measures.md` says the model means the weakest in the
registry, not the newest, and users switch models in Settings. A hardcoded
`opus-5` and `gpt-5.6` list would answer that by baking a model judgment into
code. `CLAUDE.md` bans that, and it rots at every release. The opt-in flag
answers it instead, and the re-read rate then measures which models cope.

Running it across two providers is deliberate. Anthropic uses explicit
`cache_control` breakpoints and OpenAI caches prefixes automatically. A win on
both is therefore a win from volume, not from breakpoint placement.

**One block rather than two.** It reuses a shipped tool with the exact semantics
wanted, and keeps a single rewritable thing at the tail. It also renders the
model's working set where the user can see it. Observability is the point of an
experiment, and hiding it later is a CSS change.

## Consequences

- **Both the saving and the cost are counted in rounds.** From round 2 the
  one-time payload is not re-read, and a cached read is where its size is paid
  again. A rewrite is paid per `todo_write` call, which also tracks rounds. So
  break-even is rounds per thread against `TodoListWritten` events per thread.
- **A long single-turn run is the best case, not the worst.** Dropping P tokens
  at round 2 saves `0.1 x P` on every round after it. Memory recall alone is
  capped at 50,000 chars, about 20k tokens, so a 200-round nightly saves on the
  order of 400k from that section. Round 2 itself is close to a wash, rewriting
  a small tail instead of re-reading a large one.
- **Both line items are hit, and the larger one is within-turn.** Boundaries are
  25.0% of the Opus bill and within-turn cached reads are 43.5%. Dropping the
  resume tool pairs removes the messages[0] rotation, about half the boundary
  write. Dropping memory recall and the conversation history from round 2
  shrinks what every later round re-reads.
- **The amnesia case is silent to the model but not to the measurement.** A
  thread running the flag with many rounds and zero `TodoListWritten` events is
  the signature. That count is already in the metric set, so it is countable
  rather than only visible in worse answers.
- **The first call is the worst-informed moment to curate.** The opener is often
  one line, and the model is deciding what matters about a whole workspace
  against it. Rewritability and the recovery tools make a bad harvest fixable
  rather than avoided.
- **Memory recall answers the question that was asked.** Keeping the first call's
  recall is free, because it already runs. But recall happens to a turn, and it
  is a query result rather than a snapshot, so it misses a later turn about
  something else.
- **One switch, so the comparison is before against after.** That confounds with
  anything else that changed in the same window. Accepted because the cost
  signal is large and direct: dropping memory recall removes about 20k tokens
  from every round. The quality signal is a judgment call at any scope.
- **ADR 0084's tier is left exactly as 0084 left it.** Nothing moves into the
  cached system block, so its byte stability and its
  `two_threads_in_one_workspace_share_the_system_block` guard are untouched.
- **The mode is reversible per turn.** Every request rebuilds the prompt from
  events, so the flag changes only how the next prompt is assembled. There is no
  migration and no state to rescue.
- **The re-read path exists but is a scan, not a dereference.** `query_events`
  takes `event_type`, `thread_id`, `since`, `until`, `limit` and `byte_limit`,
  with no id argument. A pointer therefore resolves as a newest-first window
  capped at 128 KB. The scheme exists: `dismiss_from_context` already accepts
  `evt-<32 hex of the ToolCalled event id>`, which resumed tool blocks carry
  today. Two halves are missing: the id must reach the model at write time
  (above), and a read must accept it.
- **`dismiss_from_context` becomes coherent rather than redundant.** It is the
  same objective function, the model deciding what leaves the prompt. It remains
  the only way to evict something inside a turn.
- **The pinned `load_knowhow` result stays as it is.** It was a hand-rolled
  instance of "the model said this matters", and it survives the resume trim
  today. Nothing here changes it.
- **Images change least of all, and cost least.** Trim pass 0 already replaces
  image bytes with a text placeholder once the turn moves on. Only the current
  turn's user images and explicitly requested ones are pinned, and an ambient
  capture is deliberately left to expire. So the boundary behaviour is already
  what this mode asks for. At 1,600 budget tokens each, getting images wrong
  costs correctness rather than money.
- **The notes block is paid twice, and output is the expensive half.**
  `todo_write` replaces the whole list, so every edit re-emits the entire block
  as output tokens. That is Manus's one-third-of-actions cost in our shape.
  Small notes are therefore the dominant cost term, not a style preference.
  `MAX_TODO_ITEMS` caps items at 50 and nothing caps characters.
- **The flag is a temporary measure** under `.claude/rules/temporary-measures.md`,
  and needs a registry row carrying the removal condition stated above.

## Alternatives considered

**An append-only note block.** Rejected, and the reason it was proposed is the
reason it lost. It was argued on cache grounds, that mutation is the enemy. The
variable is position, not mutability: a block that changes costs only what
follows it, and this block is last. Append-only would forbid the shrinking that
is the entire point, buy nothing back, and grow without bound.

**Keep compaction and summarisation, Claude Code's model.** Rejected. Late
compaction genuinely has better information, because it knows which details
mattered. It is also the answer available to a harness whose transcript is its
only memory. We have a lossless store and an addressable handle. A wrong guess
costs a tool call, not a summarisation pass.

**A per-model allow-list.** Rejected. It bakes a named-model judgment into engine
code, and rots at every model release. It answers the weak-model question by
guessing, where the flag plus the re-read rate measures.

**A guaranteed verbatim tail as a floor.** Deferred rather than rejected. Manus
and LangChain's Deep Agents both keep the newest tool calls raw, and both report
it preserves the model's rhythm. That is a quality argument, not a memory one.
The experiment is how we learn whether it is needed here.

**Curating asynchronously, Letta's sleep-time answer, or a planner sub-agent,
Manus's.** Both are the right answer to the cost objection. Both are more
machinery than a first experiment should carry. If the re-read rate is healthy
and the token cost is not, the next move is off the hot path. That is not
abandoning the design.

**Two blocks, with the notes invisible.** Rejected for now. It separates concerns
cleanly, at the cost of a second tool schema in the tools tier. It also costs two
rewritable things at the tail, and the window into what the agent believes it
needs.

**Dropping active context with everything else.** Rejected. It is the only
section with no recovery tool. "The user is looking at this right now" is not a
fact the past holds.

**Moving workspace-shaped sections into the cached system tier.** Rejected, and
it was in an earlier draft. ADR 0084 admits a section that is a function of
workspace state, and by shape the file listing qualifies. By frequency it does
not.

The tier is about 23k tokens, shared across every thread in the workspace. One
file write by any thread invalidates it for all of them at 1.25x. Today it costs
one thread one rebuild of a 5k block. The admission test for that tier is
frequency, not shape.

What was left after removing the listing was a few hundred tokens of profile,
credentials and account state. Moving those up saves their own size once per
turn boundary, which is not worth the judgment calls it needs.

**Dropping the file listing at round 2.** Rejected here, and ADR 0086 removes it
altogether instead. A first-call listing is the wrong shape for a section that
changes constantly: it is stale within minutes and the agent gets no signal that
it went stale. Taking it out of the prompt entirely is the coherent version of
the same instinct, and it is unconditional rather than tied to this flag.

**A turn-gap note for chat and trigger threads.** Rejected. A coding agent gets
one because it owns a change whose lifecycle the user drives from outside, and
`--resume` replays only its own conversation. A chat thread owns nothing of that
shape.

Everything it could miss is already covered. Workspace state is rebuilt in the
every turn and is therefore current. Its own async work pushes its own wake:
`ChildThreadCompleted`, `BackgroundBashCompleted`, an event-wait delivery, an
answered question. What remains is other threads' activity, and **cross-thread
awareness is pull, never push**. A thread is not briefed on what its siblings
did, and reaches for `query_events` if it cares.

## Deliberately left open

- **The floor.** Whether the harness ends up guaranteeing any verbatim retention
  is decided by the experiment, not by argument here.
- **Whether the notes stay visible.** They render so the experiment can be
  watched. Whether that is the right permanent surface is a separate question.
- **Read-by-id on `query_events`.** Named as one half of the gap, not specified
  here.
- **A character cap on the notes.** `MAX_TODO_ITEMS` bounds items and nothing
  bounds length. Whether a cap is needed follows from the measured output cost.
- **Which image handle replaces `thread:N`.** A stable id on the image, or the
  `save_thread_image` artifact path. Decision 11 names both and picks neither.

ADR 0087 says which of those five its eval answers, and which it cannot. The
floor and the character cap are answerable. Visibility and the image handle are
not, and read-by-id gets evidence rather than a decision.

Two more have no record anywhere else, so they are listed here to be picked up
rather than because this decision needs them.

- **Within-turn `dismiss_from_context` is unsettled.** It is the only lever that
  touches a long single-turn run's growing prefix, which this mode does not
  help. Decision 9 makes it newly usable whether or not anyone plans for it.
- **The two largest fixed blocks were never examined.** The tools array is 113
  schemas on every request of every thread, and the system prompt is about
  22.9k tokens. Both are read at 0.1x every round and neither came up.

  **ADR 0088 closes this, and corrects two of its numbers.** The array is 72
  schemas and 27,175 tokens, and the system block is 21,668. Together they are
  42.1% of every request and 32.2% of the bill. 0088 also finds both blocks
  close to optimal on size, so the item is answered rather than actioned.

The `prompt-cache-first-of-turn-miss` investigation in
`docs/temporary-measures.md` carries the remaining boundary question. ADR 0088
retires the ~9,200 unmatched tools tokens as an arithmetic artifact, and leaves
the investigation the larger fact: 58.6% of boundaries read nothing at all.

## Amendment, 2026-08-20: the model releases every body, and the engine releases none

ADR 0087's first arm measured the lean arm costing 22% MORE than the control.
The cause was position, not volume. The mode dropped memory recall from 63% of
the way back, so the 28,453 tokens behind it were re-created to save 1,365.

Two design errors sat under that. The engine dropped what the model had not
chosen, on a round number, and it kept what the model HAD chosen: a
`load_knowhow` body persisted for the life of the thread. Section ordering then
put the drop in front of everything durable.

The fix reverses both, and `docs/plans/2026-08-20-model-curated-context-mode.md`
is its record. Everything droppable becomes a *body* with a *handle*. The bodies
move behind a cache seam at the end of the user message. A *context ledger* in
front of them names the way back to each. Nothing leaves on a schedule.

### What it changes above

**Decision 2 retires.** No section leaves on a round number. Round 1 and round
30 carry the same set, less whatever the model released.

**Decision 4 retires with it.** Memory recall is a body the model releases, not
a first-call-only fetch. It runs per turn as it always did, and each run has its
own address. Releasing a recall therefore stops it being re-read for the rest of
that turn, and the next turn recalls afresh.

**"The pinned `load_knowhow` result stays as it is" reverses.** It was the one
thing the model chose and could not un-choose. Its body is now a body like any
other, addressed by its `load_knowhow` call. The model releases it when the
phase it was fetched for is over.

**Decision 3 is untouched**, and is the one thing still on the engine's
schedule. The previous turn's tool pairs go at the boundary, as they always did.

**Decision 9 earns a second job.** A live tool result states its own address.
`dismiss_from_context` now takes that address to stub the result mid-turn. That
makes decision 8's "the only way to evict something inside a turn" true rather
than aspirational.

**Decisions 1, 5, 6, 7 and 10 through 14 stand unchanged.** Decision 5 is
stricter now rather than looser. Its recovery table is rendered into the ledger
as well as the prompt, and a test resolves every row's tool against the live
registry.

## Amendment, 2026-08-21: a body is dismissed by default, and the model keeps what it wants

The amendment above shipped in full and the mode is inert. ADR 0087's pilot run
`901db33b487047a697db3625d8c84021` recorded one `ContextDismissed` across eleven
lean threads, and `repeat_recoveries` of 0 on all 22. Per round the two arms
were within 1.7% of each other, at $1.083 against $1.102.

The mechanism worked and the incentive did not. Releasing a body gives the model
nothing it can see, and holding one costs it nothing it feels. So the default
wins every time, and opt-in curation does not happen.

The fix inverts the default, and
`docs/plans/2026-08-21-persist-on-demand-context-mode.md` is its record. An
assembled body rides the round it arrives on and then leaves. It stays only if
the model says `keep_in_context`. The ledger still names the way back to
everything, so nothing becomes unreachable.

### What it changes above

**Decision 1 keeps the pen and loses the monopoly.** The model still curates,
and curation is now an override rather than the whole channel. The engine states
a default, and one call per body overrides it.

**A fixed engine rule is back, and it is not decision 2's.** That rule dropped
two named sections on a round number, and it dropped what the model had never
chosen. This one runs at the round 1 boundary and drops the unkept set. The
difference is that asking is now one call, and every unkept body has a row in
the ledger naming what fetches it back.

**Decision 4 stays retired.** Memory recall still runs per turn, and each run is
a body with its own address. The inversion changes when that body leaves, not
how often the recall happens.

**Decision 3 is untouched**, and still owns the previous turn's tool pairs.

**Decision 5 is what makes the inversion safe.** Under an opt-in default a
missing recovery route cost only what the model chose to release. It now costs
whatever the model failed to keep, which is most of the set. The recovery table
and its live-registry test move from a guard to the load-bearing part.

**Decision 9's second job stands, and gains a limit.** A live `tool_result` is
still stubbed in place by `dismiss_from_context`. It is the one home the
inversion does NOT reach: it stays opt-out.

The arithmetic permits the aggressive rule and the semantics refuse it. Stubbing
an old result beats holding it whenever the stub is under 8% of the body, and a
real stub is near 1%. But a tool result is work the model deliberately asked
for, usually in the middle of using it. A re-fetch costs a whole round, where an
assembled body was pushed at the model unasked.

**Decision 14 stands, and now covers the keep.** Nothing forces a note, nothing
checks for one, and nothing forces a keep either. A model that keeps everything
pays roughly what the mode cost before. A model that keeps nothing and needed
something pays a recovery round, which `repeat_recoveries` counts.

### What it costs, honestly

The do-nothing outcome moves from 3.0% of the request to about 24%: memory and
history at 6.5%, knowhow at 14.9%, and the prior turn's tool pairs at 3.0%.
Realized over a thread it is `24% x (R-1)/R`, so about 22% at ten rounds and 12%
at two. The ceiling is unchanged at roughly 53%, and reaching it still needs
`dismiss_from_context` for the live results.

Those shares come from 47,482 `ContextCaptured` payloads in one workspace, on
Anthropic only. They are a measurement of one tree of threads rather than a
constant.

## Amendment: the v3 default is on probation

The first eval run at the context ceiling, `07e4aa2ef0bc4317952150e4e363f433`,
recorded zero `ContextKept` and zero `ContextDismissed` calls in 206 rounds.
[ADR 0103](0103-context-trim-passes-and-the-persist-on-demand-verdict.md) audits
why, fixes the blind trimmer that destroyed the lean arm's context, and sets out
the cache arithmetic against persist-on-demand.

That arithmetic says a wrong drop costs about 32 rounds of the saving it bought.
So the model must be right about 97% of its drops to break even. The default is
not flipped here. The next run holds every other variable and answers whether
the model curates at all once the trimmer stops erasing its context.
