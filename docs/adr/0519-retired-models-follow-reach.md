# 0519: A retired model's successor runs only where this install reaches it

- **Status**: Accepted
- **Date**: 2026-10-18
- **Amends**: [0418](0418-retired-models-migrate-to-a-successor.md) (the
  router follows reach, and a stored id names a lineage) and
  [0403](0403-not-served-models-leave-default-selection.md) (a stored pick
  may move within its lineage, never across)

## Context

Migration `20261018074151_retire_superseded_models_and_seed_their_successors`
retired `claude-haiku-4-5` with `claude-haiku-5-5` as its successor. A
Vertex-only install lost every background task the same day. Vertex answered
404 for Haiku 5.5 in every region on that project, and still served Haiku 4.5.

Two mechanisms stranded it:

1. **The router always walked to the end of the successor chain.** It never
   asked whether the successor answers, so setting a preference back to Haiku
   4.5 changed nothing.
2. **The migration rewrote every stored pick to the successor.** SQL cannot
   know which provider serves what, so nothing walked back.

A third defect hid the first. The not-found answer was recorded against the
model the caller named, so the one model that worked was marked not served.

The migration's comment says Haiku 4.5 is past its retirement floor on
Vertex. That is wrong: Vertex still served it. The migration has applied on
dev installs, and sqlx refuses to boot on an edited applied migration, so the
correction lives here.

## Decision

**A stored model id names a *lineage*: the model and every model linked to it
by `successor`.** Every call resolves its named model within the lineage, at
call time, by reach:

- **Newest generation first, the named model last.** The first one reachable
  runs. Reachable means the route the router would take is configured, and
  that provider has no not-found answer for the model inside the six-hour
  window.
- **A stored pick or a turn's model may step back.** With nothing newer
  reachable, its predecessors are tried, nearest first. This undoes the
  migration's rewrite without knowing it happened.
- **A default or recommended model never steps back.** With its lineage
  unreachable, the call meets the not-found answer and the purpose moves to
  its next recommended model (ADR 0403).
- **Nothing reachable sends the end of the chain**, which fails loudly.

One resolver, `model_registry::resolve_generation`, answers for the router,
default selection and Settings. `ModelSource::generation_rule()` maps the
source of a selection to its rule. One `NotServedLedger` holds the not-found
answers; the router marks it, and it is seeded from `ModelNotServedObserved`
at boot. On a not-found answer the router records the provider and model it
actually sent, resolves again, and retries once if the answer changed.

**The command judge follows the same rule.** A judge stored on Haiku 5.5 steps
back to Haiku 4.5 where 5.5 is unreachable, and Settings names the step.

## Rationale

**The provider decides what is reachable, so the call time decides.** A
migration runs where nothing knows the provider. The router meets the answer
on every call, and the ledger makes that answer cheap to keep.

**"Every configured route" reduces to the route the router would take.** ADR
0247 forbids moving a call to another route of the same model, so that route
is the only one a call can reach.

**A step back restores the user's choice, a step across replaces it.** A
predecessor is the model the user or the install held before the rewrite.
Another lineage is a model nobody chose, which ADR 0403 already forbids for a
stored pick.

**A default never steps back** because ADRs 0406 and 0448 require a measured
model for the judge's default and the wake decision. A predecessor was never
measured for them. The next recommended model was.

**A reachable successor still wins**, as ADR 0418 requires. That ADR exists
for a provider that silently reroutes a retired id behind a 200. No 404 ever
reveals that, so "hold the retired id while it is served" would bring the
failure back. The retired id runs only while its successor is unreachable, and
Settings says so.

**The judge steps back like every stored pick.** ADR 0406 measured Haiku 4.5
as the weaker judge. A weaker judge that answers still beats one that fails
every call to an approval card.

## Consequences

- `retire_model` stays a one-line retirement. A retirement can no longer
  strand an install whose provider has not enabled the successor.
- Settings names what runs when it is not the named model: the successor, an
  earlier version, or the retired id held. `GET /api/v1/models/background`
  serves it as `generation`.
- A chat turn's step is not drawn in the transcript. `served_model` already
  records the model that answered, and the router logs the step.
- The router's own not-found marks are a cache. A restart re-learns each one
  with one fast 404 and an in-call retry, unless an auxiliary call persisted
  it as `ModelNotServedObserved`.
- The compactor prices, routes and records its spend for the generation that
  runs, so a backfill estimate names the model that will answer.

## Alternatives considered

**A restoring migration.** It would put the retired id back where the
successor is not served. Rejected: SQL cannot know where that is. Only
preferences carry a timestamp that proves a rewrite. Several retired ids share
one successor, so it could not tell which to restore.

**Hold an explicit pick of a retired id while it is served.** Rejected: a
provider that reroutes behind a 200 never reports the id as unserved, which is
the case ADR 0418 exists for.

**Check every configured route, not the router's.** Rejected by ADR 0247: a
call never moves between routes of one model, so only the router's route can
answer it.

**Keep the judge from stepping back.** Its default is measured, so a pick that
steps back runs a weaker judge. Rejected at approval: a judge whose lineage is
otherwise unreachable would fail every call safe to an approval card, which is
the outage this fixes.
