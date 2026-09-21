# 0220: A typed judgment is a sibling of a chat completion, and Jev is opt-in beside the path it does not replace

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

Two auxiliary model calls in Lucidos are pure classification. The command
guard's judge sorts one command into a risk lane and a side-effect category
(ADR 0002). Query classification answers three yes/no questions in front of
memory retrieval. Both are written the same way: a rubric prompt ending in
"return strict JSON", then a tolerant parse of whatever came back.

TypeSafe's System One model, Jev, answers typed questions instead. A **Choice**
returns one option plus a probability for every option. A **Noul** returns the
probability that a condition holds. A **Score** returns a position on ordered
levels. Independent questions over one state batch into a single request.

## Decision

A typed judgment gets its own trait, `JudgmentProvider`, beside `LlmProvider`
rather than inside it. Jev is its only implementation.

Each classification call site keeps the prompt-and-parse code it has today, and
that code stays the default. One preference per site (`judgment_command_guard`,
`judgment_query_classification`) opts it over to Jev.

## Rationale

**The interfaces are not the same shape.** `LlmProvider::chat` takes messages,
tools, a system prompt and a token callback, and returns text plus tool calls.
Jev takes a state value and named questions, and returns typed answers. The only
way to fit Jev to `chat` is to encode the questions into a fake user message and
parse text back out. That discards the probability distribution and the
confidence, which are the whole reason to use it.

**The distribution is what moves logic out of the prompt.** ADR 0002's rule is
"when unsure between safe and a danger lane, pick the danger lane". Today that
is a sentence in a rubric, and a model either honors it or does not. With a
distribution in hand it becomes a threshold in Rust, pinned by a test that needs
no network. The same holds for query classification, where the tie now goes to
loading context.

**Keeping the old path is cheaper than writing a generic fallback.** An earlier
draft added a `ChatJudgmentProvider` rendering typed questions into a prompt, so
every site could speak one interface. It would have replaced two working, tested
code paths with one new untested one. One of those surfaces is the safety gate
over the agent's own shell commands. The code already in the tree is a better
fallback than any we could write.

**A credential is not consent.** The preference gates the switch, not the key.
So storing a TypeSafe credential changes no behavior, and a key present for one
site does not move the other. That makes the no-change promise true by
construction rather than by test.

## Consequences

- Jev never appears in the **chat** model picker. It is not a chat model, and
  `/health`'s `configured_providers` stays the list of backends that can hold a
  conversation. There is no `ProviderKind::TypeSafe`.
  [ADR 0224](0224-typesafe-jev-is-picked-as-a-model.md) later narrowed this
  clause to the word "chat": Jev IS a row in the model picker at the two sites
  below, because that is where a backend is chosen. Everything else here holds.
- It does get a **builtin proxy**, `lucidos.proxy('typesafe')`, so an app can
  ask a typed question without holding the key. That needs no `ProviderKind`: a
  resolver reads a credential by name and pins a base URL. It is the first
  builtin proxy that is not a model-registry row, so `proxy_builtin`'s doc now
  says so.
- The Settings surface is three controls, not one. The key sits with the other
  API keys, and each site chooses its backend beside the feature it changes. See
  `docs/plans/2026-09-19-jev-settings-controls.md`. Those two per-site controls
  started as switches and are model rows now, per ADR 0224.
- **A fourth control was added later: a plain on/off switch on the provider
  row**, `provider_enabled_typesafe`, so that row behaves like every other one
  on the page. It is an addition to this decision and not a reversal. Jev still
  has no `ProviderKind`, still appears in no CHAT model picker, and still never
  reaches `configured_providers` or `llm::provider_build`. What the switch buys
  is interaction shape: one master switch above the two per-site preferences,
  absent meaning on, off leaving the stored key alone, and `jev_for` returning
  `None` while it is off. The row derives its own state from a stored
  `typesafe` credential, which is what stands in for the `/health` entry this
  ADR denies it. See
  `docs/plans/2026-09-19-typesafe-provider-row-master-switch.md`.
- Two code paths exist per converted call site. That is the cost of the
  guarantee above, and it is paid in full: a change to a rubric prompt has a
  sibling question set that should change with it.
- `SUB_QUERY_PROMPT` repeats the query-writing guidance from
  `QUERY_CLASSIFICATION_PROMPT`, for the same reason. The two are marked to be
  edited together.
- On the Jev path the permission card's sentence is derived from the lane and
  the category, because Jev writes no prose. It is more consistent and less
  specific than the model-authored one. The chat path keeps the model's.
- A Jev failure falls through to the rubric prompt. A Jev **timeout** does not:
  a user is waiting on the permission card, and a second full deadline behind
  the first is worse than the static fallback.
- Adding a third classification call site is a question set plus a preference,
  with no change to the transport.
- **The agent got its own route later, and it is not a call site.**
  [ADR 0223](0223-a-typed-judgment-the-agent-asks-for.md) adds the `judge`
  tool, where the questions come from the model rather than from Rust. It takes
  no preference of its own, because it replaces nothing: a configured provider
  is the whole condition, exactly as it is for `generate_image`. The two sites
  above keep running their own path while it is offered, which is what keeps
  the no-change promise true.

## Alternatives considered

**Make Jev an `LlmProvider` and register it in the model registry.** It would
have needed no new trait, and Settings would list it beside every other model.
Rejected because the adapter has to serialize questions into prose and parse
answers back out, which throws away the distribution and the confidence. It also
offers the user a model that cannot hold a conversation, in a picker whose
entries all can.

**Replace the prompt path outright, with no preference.** Simpler, one code path
per site, no duplicated guidance. Rejected because the first surface it touches
is the gate over the agent's own shell commands. The workspace would also depend
on a third party for a decision it already makes on a model it pays for.

**Switch on the credential being present, with no preference.** One less knob.
Rejected because installing a key would silently move the safety gate to a
different backend. That is exactly the surprise a safety gate must not have.

**Move fact extraction and title generation too.** Rejected on capability, not
taste: both produce new text, and Jev produces none.
`ExtractedFact.importance` is a Score in shape and remains a candidate. Scoring
it means one question per extracted fact, so it needs measurement first.

**Retry a 429 or 529 inside the provider.** The TypeSafe docs recommend backoff.
Rejected here because both callers already hold a fallback answering in one
round trip. A retry also sits inside a path a user is blocked on.
