# 0809: Release blockers live in the Software Factory app

- **Status**: Accepted. Supersedes [ADR 0703](0703-release-tag-blocker-ledger.md).
- **Date**: 2026-11-25

## Context

ADR 0703 put release blockers in `release-blockers.tsv`, a tracked file at the
repository root. Recording, fixing or waiving a blocker became a commit. A
commit to Lucidos source needs a coding-agent thread, hardening and an Apply.
That is too slow for the moment a blocker is found, which is usually in a chat.

The owner decided that blockers live in the Software Factory app, the
workspace content that holds the factory's work. 0703 had rejected the
workspace event store for five reasons, and each needs an answer.

## Decision

1. **A blocker is the app's own domain events.** Seven types:
   `ReleaseBlockerLedgerOpened`, `ReleaseBlockerOpened`, `ReleaseBlockerFixed`,
   `ReleaseBlockerReopened`, `ReleaseBlockerWithdrawn`, `ReleaseBlockerWaived`
   and `ReleaseBlockerRecordVoided`. Each carries `v` and a `ledger`.
2. **A ledger belongs to one repository**, keyed by its root commit, and is
   opened by a device tap. A release reads the workspace it runs in.
3. **The gate stays in the repository.** `release_blockers_check` reads every
   event of those types through `lucidos events query` and `count`, projects
   them, and judges the result against the cut exactly as before.
4. **A fixed blocker passes only when its commit is an ancestor of the cut.**
5. **Every input error refuses**: no CLI, an unreachable engine, an incomplete
   read, no ledger for the cut's root, an unusable record, an unknown `v`.
6. **Withdrawals, waivers, voids and ledger openings count only from a device
   actor.** The engine stamps the actor and replaces any value a caller
   writes. An agent's record of these types is listed as unconfirmed and
   changes nothing.
7. **A waiver names one version and a reason.** RELEASE GATE, STILL OWED, the
   local tag message and the waived count all print it.
8. **Nothing reaches the mirror but the trailer.** `Release-Gate:` keeps its
   format. Its `ledger=` digest now covers the snapshot the gate judged.
9. **Every door still runs the gate**, including the `release-gate` job in
   `release-tarballs.yml`, which checks the trailer and needs no workspace.
10. **The engine gains nothing.** Event types are free-form, the actor stamp
    exists, and the reader is repository code.

## Rationale

The record moves to where blockers are found, and the judgment stays where it
is reviewed. Any agent can open or fix a blocker from a chat in seconds. The
decisions that let a release through without a fix in the cut need a person's
tap. Those are a withdrawal and a waiver.

An agent's fix record is trusted up to ancestry, with no review. The gate
checks only that the named commit is in the cut, not that it fixes the
blocker. Under 0703 the same claim went through a reviewed commit. The owner
accepts that trade for speed, and RELEASE GATE prints every fix commit for
inspection.

0703's objections, answered:

- **The work item is an app concept.** So is a blocker now. The engine names
  neither.
- **Several workspaces develop one repository.** A ledger has one home, opened
  by a device tap and keyed by the root commit. A release elsewhere refuses.
- **The GitHub runner has no store.** It never needed one: it checks the
  trailer the local gate wrote.
- **The tracker's status is hand-typed and stale.** The gate never reads work
  items. A fix is a commit, checked against the cut.
- **`WorkItemsChanged` is being replaced.** Blocker events are separate types.

A device actor is stronger evidence than git authorship. 0703 conceded that
git cannot tell an owner's instruction from an agent's choice. The engine
stamp can tell a tap on a paired device from an agent's emit.

## Consequences

- Recording a blocker is one event, from the app, its script or a chat. It
  needs no commit, hardening or Apply.
- A release now needs the workspace engine. With the engine down, or from
  another workspace, the gate refuses. The TSV needed only git.
- A record the gate cannot use refuses every release. A later valid record
  supersedes it, or a device tap voids it. A void may name only a rejected
  record, so it cannot hide a real blocker.
- The ledger's audit is the event log, with actors and times. Each release's
  tag message carries the full snapshot and the ledger sequence, so the ledger
  at each release survives in git on `origin`.
- A blocker raw-emitted in a workspace that is not the home is never read. The
  app's script refuses there, because that workspace has no ledger.
- In a workspace with several members, any member's device counts.
- `release-blockers.tsv` stays in the release tree's exclusion list, because
  an older cut can still contain it.
- Another factory adopts the gate by running the app's blocker module and
  calling `release_blockers_check` from its own release step.

## Alternatives considered

- **Keep the TSV (ADR 0703).** Rejected by the owner: a blocker found in a
  chat should not need a coding-agent thread and an Apply.
- **A blocker flag on a work item.** The work item model refuses hand-typed
  state on items, and that model is mid-migration.
- **Derive "fixed" from a linked work item's Released stage.** Deferred. The
  stages are not built, and an item with several streams has no single fix.
- **Let the app compute the verdict.** The release verdict would then be
  workspace code that no review sees. The repository owns the release.
- **One event type with a state field.** Granular types let triggers and future
  event hooks target one decision, such as a waiver.
- **Read every reachable workspace.** An offline remote gateway would either
  wedge every release or be skipped silently.
- **Refuse bad records at write time with an event hook.** Deferred: a domain
  emit is not yet an asking site. The gate must check anyway, since a hook can
  be disarmed.
- **Waivers stay as git rows.** Two sources for one record. The device actor
  is the stricter path, and it keeps the record in one place.
- **A `lucidos release-gate` command or a `--waive` flag.** Still rejected, for
  0703's reasons.
