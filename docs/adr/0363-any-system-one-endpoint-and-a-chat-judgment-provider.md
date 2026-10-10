# 0363: A judgment provider is any System One endpoint or a chat model, and sites drop their own fallbacks

- **Status**: Accepted
- **Date**: 2026-10-04

Record: `docs/plans/2026-10-04-tree-memory-module-and-the-home-thread.md`.
Supersedes the fallback clause of
[ADR 0220](0220-jev-is-a-judgment-provider-not-an-llm-provider.md) and amends
[ADR 0224](0224-typesafe-jev-is-picked-as-a-model.md).

## Context

ADR 0220 gave typed judgments their own trait, `JudgmentProvider`, with Jev as
its only implementation. Each site kept its prompt-and-parse code as the
default, and one preference per site opted it over to Jev.

Two things changed. First, System One stopped being one vendor. Cloudflare's
Clef and Clef-flash speak the same API, and switching means changing the
endpoint and the model. Open models (Kev, Strands Decider) can be hosted
locally.

Second, ADR 0362 adds `find`, a site with no prompt-and-parse path to fall back
to. It walks a summary tree by asking choice questions over its lines. Without
a System One key, it needs a chat model that answers typed questions.

ADR 0220 considered a chat judgment provider and rejected it. Its reason: two
working, tested paths would give way to one new, untested one, and one of them
is the command guard.

## Decision

**One System One provider, many endpoints.** `JevProvider` generalises to a
provider with a base URL, a model and a credential. Jev, Clef and Clef-flash
are seeded rows, and a self-hosted endpoint is a row the user adds.

**A chat model is a judgment provider too.** It answers the same `Question`
types through structured output, and reports a distribution over the options.

**Every site speaks `JudgmentProvider` alone.** The command guard and query
classification delete their prompt-and-parse fallbacks. `find` joins as a new
site. Each site's model picker lists chat models and System One rows side by
side, as ADR 0224 already does for Jev.

**The command guard moves only behind its own tests.** Its existing test suite
must pass on the chat judgment provider before its rubric path is deleted. Its
fail-safe stays a Rust threshold: a missing or malformed answer resolves to
`IrreversibleDanger`.

**The default is still a chat model.** An untouched workspace runs every site
on a chat model. Storing a System One credential changes no site.

## Rationale

**ADR 0220's objection is answered by its own test suite.** The risk was
replacing tested code with untested code. Porting the guard's tests to the new
provider, and passing them first, makes the new path the tested one.

**Two fallback styles would grow into many.** Each new site would choose
between owning a prompt-and-parse path or speaking the trait. One interface
means a site is written once and runs on any backend.

**The distribution still decides in Rust.** A chat provider's probabilities
are less calibrated than a System One model's. The threshold stays in code, so
a weaker distribution moves no rule into a prompt.

**Endpoints are rented models, not surfaces.** Each one sits behind our trait
and our picker, per rule 1 of `docs/philosophy.md`.

## Consequences

- `classify_on_chat` and the command guard's rubric fallback are deleted.
- Picking a System One row for a site sends that site's state to that vendor.
  For `find`, that state is summary tree lines.
- A self-hosted System One model keeps judgments on the user's own machine.
- Score stays unimplemented until a site asks one.
- A row carries a full request URL rather than a base URL. Workers AI puts the
  account and the model in the path, so one base cannot serve Jev and Clef.
  The Clef token's credential scope carries the account.
- A failed System One call falls back to the site's chat model, inside the
  site's one deadline. This is one generic fallback, not a per-site path, and
  it keeps a vendor outage from turning every judgment into its default.
- The chat provider reads its answer through one tool whose schema the
  questions define, because no provider here exposes `response_format`. A
  reply it cannot read, an empty one included, is an empty answer set rather
  than an error. So the guard asks where the rubric path used to fall back to
  the static list.

## Alternatives considered

- **Keep each site's fallback, chat provider for `find` only.** Lost: two
  fallback styles side by side, and every new site picks one.
- **Encode questions into a chat message per site.** Lost for the reason ADR
  0220 gives: it throws the distribution away.
- **A separate trait per vendor.** Lost: Clef speaks the same wire as Jev, so
  a second trait would duplicate the first.
