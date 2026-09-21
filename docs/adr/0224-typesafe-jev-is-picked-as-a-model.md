# 0224: TypeSafe Jev is picked as a model at its two call sites, not by a switch beside one

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

[ADR 0220](0220-jev-is-a-judgment-provider-not-an-llm-provider.md) gave each
classification call site a preference choosing its backend, and Settings
rendered that preference as a switch. The command guard therefore showed a
`Judge model` dropdown with a `Judge on TypeSafe (Jev)` switch under it. Query
classification showed the switch alone, since it had no model row at all.

Every other provider in Lucidos is set up once: store a key, then pick one of
its models wherever a model is picked. TypeSafe was the only one configured
twice, and the second control was a different shape from the first.

## Decision

TypeSafe (Jev) is a row in the model step of a judgment site's picker, beside
the chat models. Both per-site switches are deleted.

Query classification gains the model selection it never had,
`model_query_classification` and `reasoning_query_classification`, falling back
to `model_memory` while unset. It also gains a `ContextPurpose` of its own, so
it stops sharing fact extraction's.

## Rationale

**One control per decision.** Which backend answers and which model it answers
on are the same question asked twice. A user reading `Haiku 4.5` in a dropdown
with a switch under it cannot tell which one the engine obeys, and the answer is
the switch.

**The shape is already there.** Jev offers no reasoning tiers, so the picker
settles on the model step in one tap, exactly as an image model does. Nothing
in `useModelSelection` needed changing.

**A model whose provider is unconfigured is absent, not disabled.**
`chatModelOptions` already filters by configured provider, so the Jev row simply
does not appear without a stored key. That deletes the whole blocked-reason
apparatus the switch needed, and it keeps ADR 0220's promise by construction: a
workspace with no TypeSafe key sees exactly the list it saw before.

**Splitting query classification out was the price, and it was owed anyway.**
`aux_purpose.rs` carries a standing invariant of one purpose per auxiliary model
preference, and classification was the last job still sharing `model_memory`
with fact extraction. Without the split its row would have offered one model,
which is not a picker.

## Consequences

- ADR 0220's consequence "Jev never appears in the model picker" narrows to the
  **chat** model picker. Jev still holds no conversation, has no `ProviderKind`,
  never reaches `configured_providers`, and never enters `llm::provider_build`.
- The `judgment_*` preferences are unchanged in name, values and meaning. No
  migration, and a workspace already on Jev keeps running on it.
- Picking Jev writes one preference and leaves the stored model alone, so
  switching back restores the model the user had rather than a default.
- Picking a chat model writes two preferences, the judgment key first, because
  that is the one deciding which backend runs. Neither can strand the other:
  `savePreference` never rejects, so a refusal toasts and a transient failure
  parks for the resume flush, exactly as they do for every other setting.
- A site already on Jev always sees the Jev row, key stored or not, and with the
  master switch off or on. The key can come from `TYPESAFE_API_KEY`, which the
  page cannot see, and a selection with no way back is worse than an odd row.
- With the master switch off, the row shows Jev and says the site is running its
  chat model. The stored pick is still Jev, so the field cannot honestly show
  something else.
- Fact extraction keeps `model_memory` outright. Query classification inherits
  it while its own key is unset, which is the same shape the conversation
  summary has.
- A capture for query classification is now distinguishable from one for fact
  extraction on the wire.
- `ContextPurpose` grew a variant, so the thread-event wire contract was
  regenerated.

## Alternatives considered

**Keep the switch and leave query classification alone.** No engine change at
all. Rejected because it leaves both screens in the shape the maintainer
objected to, and the second screen has no model control to be consistent with.

**Give query classification a two-row dropdown**: the memory-extraction model by
its real name, plus Jev. Frontend only, no new preference. Rejected on two
counts. A dropdown offering two models beside siblings offering five reads as
broken rather than consistent. And its first row would name a model this row
cannot change.

**Register Jev in the model registry as an `LlmProvider`.** Already rejected in
ADR 0220, and still rejected: the adapter has to serialize typed questions into
prose and parse answers back, discarding the distribution. This decision offers
Jev only where the call IS a typed judgment.

**Offer Jev in every background model row.** Rejected on capability. Title
generation, image description, fact extraction and the conversation summary all
produce new text, and Jev produces none.

**Disable the Jev row rather than hiding it when no key is stored.** Rejected
because no other provider's models are shown greyed out, and the row would have
to carry the blocked-reason sentence this change removes.
