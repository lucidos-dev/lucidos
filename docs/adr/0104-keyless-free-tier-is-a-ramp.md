# 0104: The keyless free tier is a ramp: opt-in, never the default, and it states its terms at the switch

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

Lucidos answers nothing until a provider credential exists. Every first-run user
therefore meets a key field before they meet the product.

OpenCode's Zen relay serves a set of free models anonymously, over the plain
OpenAI wire format. A probe established the facts rather than assuming them: a
real two-turn Lucidos loop, through our own `OpenAiProvider`, with our real
tools array, against the six free models Hermes Agent verified. Every model
returned parseable tool calls, and our SSE parser dropped nothing.

So the capability is real and nearly free to add. What needed deciding is
whether we may depend on it, and on what terms.

## Decision

`opencode-free` is a seventh provider, and it is a ramp rather than a
foundation. Four rules bound it.

1. **Off by default.** It installs from the `opencode_free_enabled` preference
   or its env fallback, never from mere presence. A workspace that never asked
   is byte-identical to before.
2. **Never the first-run default.** A pristine workspace still reports
   `llm_configured: false` and still shows provider onboarding.
3. **The terms are stated at the switch.** Requests go anonymously to a third
   party, and several free models may train on what they receive. That sentence
   renders beside the toggle, not behind an explainer or in a doc.
4. **It carries no credential and impersonates nobody.** The key is empty, so no
   `Authorization` header is sent. The User-Agent names Lucidos, which is why
   `big-pickle` is not seeded: the relay serves it only to the OpenCode CLI's
   own User-Agent.

## Rationale

**The philosophy test does not block it, and does not bless it either.** Renting
a model is rule 1's free half, and a keyless relay is a model dependency behind
our own registry, speaking our own interface. It takes no attention, owns no
transcript, and holds no state. It is a ramp in.

**What the test is silent on is the data.** A Lucidos chat turn carries the
user's workspace: files, knowhow, artifacts, the conversation. Sending that to
an anonymous endpoint whose retention is unstated is a different act from
sending a coding agent a diff. So the decision is not "is it useful" but "who
chose, and did they know". A default would answer neither. A toggle with the
terms on it answers both.

**Nothing here may be depended on.** The endpoint is undocumented for anonymous
use, the catalog rotates, and one seeded model already returned "Model is
unavailable" mid-probe. That is survivable only because the fallback is what we
already have: a user's own credential, unaffected. The tier is excluded from the
web-search chain and from the builtin proxy for the same reason. Neither should
acquire a dependency that vanishes without notice.

## Consequences

- A user can try Lucidos with no account, no key and no card, in one tap.
- A free model's answer is weaker than a frontier model's, and the picker offers
  both, so the user sees which one they chose.
- A retired free model needs a migration, because a builtin row is disable-only.
  That is the cost of declaring windows on rows instead of reading a live
  catalog.
- Reasoning tiers are measured per model on this provider, not assumed. Ox Alpha
  rejects three of the six ladder values with a 400.
- Adding the toggle also made provider preferences hot-swappable, which fixed
  `local_base_url` needing a restart.

## Alternatives considered

**The Hermes shape: keyless by default on first run.** The best onboarding by
some distance, and rejected on the data. A pristine workspace would send the
user's first prompts to a third party before they chose a provider at all. The
free catalog's retention terms are exactly the ones we cannot promise. Reopening
this needs a retention guarantee, not a better onboarding argument.

**A Lucidos-hosted free tier.** We would own the terms and the rate limits, and
we would pay for both. It also needs an account to stop abuse, which is the one
thing the keyless tier is for. A different product decision, not this one.

**A credential row with an empty value as the toggle.** It would have reused the
credential subscriber for free. Rejected: it models a switch as a secret, and it
puts an empty credential in front of the user in the credentials list. The
preference plus a widened subscriber is one more line and stays honest.

**Reading the live `/models` catalog instead of seeding rows.** It tracks
rotation, which seeded rows cannot. Rejected here because our registry is DB
rows with declared context windows, and a discovered model declares nothing. It
stays open as its own decision if rotation turns out to bite.

**Bundling a local model.** Truly keyless and truly private. It is a multi-GB
download and weak at tool calling on consumer hardware, which is most of what
Lucidos does. The `local` provider already serves anyone who wants this.
