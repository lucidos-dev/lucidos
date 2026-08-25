# 0124: A new thread carries no cross-thread conversation history

- **Status**: Accepted
- **Date**: 2026-08-25

Record: `docs/plans/2026-08-25-a-new-thread-starts-clean-and-the-summariser-goes-async.md`.

## Context

A chat send with no thread id built its `[CONVERSATION HISTORY]` from
`get_recent_messages(32, None)`. That reads as "the last 32 messages" and is
not. The limit binds the `recent_threads` CTE, so it selects the 32 most
recently active threads and returns every message in them.

Measured in the dev workspace, that is 129 user messages and 97 assistant turns
across 32 unrelated conversations. All of it rendered into a brand-new thread's
prompt: other threads' user turns verbatim to the 20,000-char budget, their
assistant turns compressed by the summariser.

That summariser call could never be kept. ADR 0102 forbids caching a paragraph
built from a global window, because its boundary would name an event this
thread's history never contains. So the turn paid for it and discarded it.

**The path is rare and was once the default.** `is_new_thread` is
`thread_id.is_none()`, and a compose draft is already a thread. So a normal new
chat sends a thread id and takes the ordinary follow-up path, where its own
history is empty. Restricted to plain top-level chat threads, the raw-new share
was 94.7% in April and 3% in August: 9 threads of 289. What remains is the
raw-new send path, such as the new-app form, plus API callers that post without
a thread id.

## Decision

**A chat turn reads only its own thread's events.** The turn drops its
`get_recent_messages` call. A trigger and a raw-new send share one guard
returning the empty history load they both already produced.

**Cross-thread continuity belongs to long-term memory.** Recall runs on every
turn including the first, ungated by thread age. Artifacts, knowhow and apps
carry the rest. No replacement window is built.

## Rationale

**Two mechanisms answered one question, and the worse one ran first.** Memory is
extracted, embedded, ranked by relevance and scoped to the user. The window was
a recency slice of whole conversations, ranked by nothing, chosen by whichever
threads happened to be active. When both are present the window is not a
fallback, it is noise with a much larger byte cost.

**A conversation is the unit a thread is named after.** Everything else in the
turn is thread-scoped: the resume tool blocks, the loaded knowhow, the todo
list, the working understanding. History reaching outside was the one exception,
and nothing depended on it deliberately.

**It made the summariser's worst call.** A hundred assistant turns from 32
threads, compressed by a model that knows none of them, into a paragraph ADR
0102 then refuses to cache. Deleting the window deletes that call, which is why
`thread_local` and its no-cache branch could go: every summariser call now
caches.

**The volume says this is safe to do now.** At 3% of new chats the blast radius
is small, and the compose draft made it small without anyone deciding to. That
is the argument for writing the decision down rather than leaving it as an
accident of the draft flow.

## Consequences

- **A raw-new send starts with no conversation history at all**, where it used
  to start with 226 messages of somebody else's. Memory recall, the workspace
  inventory, artifacts, knowhow and apps are what it has.
- **Memory extraction stops seeing cross-thread text.** The 500-char string it
  reads came from `summarize_user_topics` over the window, so a raw-new turn
  fed it other threads' user messages. It now gets this turn's message.
- **`is_new_thread` stops threading through the history builder.** The
  image-hash and staleness branches it gated are unconditional again, and the
  summariser's `thread_local` argument is gone.
- **`get_recent_messages` stays.** `/api/v1/messages` is still a caller, so the
  reader is not dead code and only a source scan can hold this boundary.
  `no_chat_turn_reads_another_threads_messages` is that scan.
- **If a raw-new chat reads as amnesiac, the fix is recall, not the window.**
  Putting a recency slice of unrelated conversations back would hide a memory
  problem behind bytes.

## Alternatives considered

**Keep the window and stop summarising it.** Rejected. It fixes the wasted call
and leaves the real problem: other threads' content in this thread's prompt,
occupying real bytes with no relevance ranking.

**Keep the window and cache the paragraph against the new thread.** Rejected,
and ADR 0102 already rejected it. The row would file other threads' content as
this thread's own, and its boundary would name an event this thread never
contains.

**Shrink the window instead of removing it.** Rejected. Any size is the same
mechanism, and picking one asks how much of an unrelated conversation belongs
here. The answer the rest of the turn already gives is none.

**Replace it with a memory query seeded from recent threads.** Rejected as
already built. Recall runs on the first turn and reads the same corpus, ranked
by relevance rather than recency.

**Leave it, since it is only 3% of new chats.** Rejected. The share is small
because the compose draft made it small, not because anyone decided the window
was wrong for the other 97%. A path that rare is also the one nobody notices
misbehaving.
