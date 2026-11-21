# 0623: A team workspace is one factory; an operator is identity without authority, and a work item is a root thread with an opaque continue key

- **Status**: Accepted
- **Date**: 2026-11-07

Builds on [ADR 0014](0014-multi-workspace-redesign.md) (no auth in our code),
[ADR 0132](0132-gateway-auth-state-is-per-gateway.md) (pairing is per gateway) and
[ADR 0297](0297-events-store-device-ids-not-names.md) (events store device
ids). Plan: `docs/plans/2026-11-07-team-workspace-factory.md`.

## Context

A team wants to run its agent work in loops: tickets picked up on their own,
some going straight to a pull request, some stopping for a human decision. We
compared a hosted factory product (Warp Factories) with one shared Lucidos
workspace that every team member pairs devices to.

A shared workspace raises five questions:

- Who may do what, once several people reach one workspace?
- How does the team tell its people apart, without accounts?
- How does Lucidos know a person who writes from a ticket or a chat tool?
- What is a unit of work, when tickets and chat threads arrive from outside?
- How does work reach the right person, and never reach nobody?

## Decision

1. **The factory is the only principal.** The workspace holds the credentials
   and does the work. People configure it and steer it. No person-level
   permission exists anywhere.
2. **An *operator* is identity without authority, kept at the gateway.**
   - An operator is a person: a name, one or more emails, and the *paired
     devices* they hold. One device belongs to one person.
   - Operators drive attribution, routing and read state. They never allow or
     refuse an action. Every paired device keeps full access.
   - The *workspace gateway* keeps operators beside its devices, in an
     append-only *gateway event log*. Devices and operators are projections of
     that log, so device auth needs no Postgres.
   - Minting a *pairing secret* asks for whom. A device relayed through a
     *gateway pairing* carries gateway A's operator in A's signed relay claim,
     with the operator's emails. B counts a relayed operator holding one of
     its own operators' emails as that operator, so one human paired to two
     gateways is one person. Ending the pairing forgets its relayed operators.
   - Engines read operators from the gateway's control API and cache them.
3. **Outside people are known by email.** A trigger may name an *actor*: the
   payload field holding the email of the person behind a fire. The engine maps
   it to an operator. A source that sends ids gets a workspace script trigger
   that resolves the id and emits a domain event carrying the email.
4. **A *work item* is a root thread and its sub-threads.** No new entity, no
   lifecycle, no source table.
   - **Input:** a trigger may give a fire an opaque *continue key*, built from
     payload paths. A fire whose key matches an open thread continues it, and
     answers its open question as free text. Otherwise a new thread starts and
     keeps the key. The Archive action releases its key. An unattended run
     that lands in Archive on its own keeps it, so the work item continues.
   - **Output:** workspace script triggers on thread events post questions
     and outcomes back to the source, each short with a link to the thread.
     The engine posts nothing to a source.
5. **A *gate* is a question card on a work item.** Gate policy lives in
   knowhow. No engine code decides when to stop.
6. **A work item has at most one *driver* and any number of *watchers*.**
   - The driver is soft: anyone can still answer any gate.
   - A gate with no driver notifies the whole team.
   - A gate its driver leaves past the team's timeout escalates once to the
     whole team. The driver keeps the work item. The timeout defaults to
     never, so a personal workspace never escalates; the team setup sets it.
   - Roles are folded at read time from events on the family root, with the
     current operators. No summary column holds them, so moving a device or
     an email re-attributes every role.
   - A notification records its audience as device ids on its event: the
     work item's driver and watchers, or the people a notice names. A
     gateway that knows fewer than two people ordinarily reaches everyone.
     Recovery of an admitted team message retains person scoping after
     membership shrinks; it never broadens that message to everyone (ADR 0649).
   - Taking over a work item, by Take or by answering its gate, tells the
     person who drove it. Archiving the root tells its driver and watchers.
7. **The team views are a *factory app*.** A Lucidos app shows work items with
   driver chips and Mine, Team and All, through the SDK. The core drawer does
   not change.
8. **Config changes are live, attributed and undoable.** No review gate on
   adding a repo or a trigger. Undo emits the inverse event. No history table.

## Rationale

- **ADR 0014 already settled authorisation.** Lucidos has no auth in its code,
  and multi-user access is an external concern. A role system would reopen it
  for one feature, and every paired device can reach a shell anyway.
- **Identity is still worth having.** A team needs to see who drives what, who
  changed a trigger, and what it has not read yet. None of that needs a
  permission.
- **Operators belong where pairing lives.** Pairing is per gateway, and one
  paired device reaches every workspace that gateway serves. A per-workspace
  operator table would repeat the mapping in every workspace and drift.
- **An event log keeps the gateway honest without Postgres.** Device auth must
  work while Postgres is down. A JSONL log replays into the same state, and
  every change has a record.
- **Operators are device groups, so ADR 0297 holds.** Events keep device ids,
  or an actor email for outside input. Moving a device or an email to another
  operator re-attributes its old events, the way a device rename does today.
- **Email is the identity sources already share.** Warp's Slack and Linear
  integrations match people by email too. It needs no OAuth sign-in per
  provider.
- **A root thread is already a unit of work.** It has state, sub-threads,
  questions, changes and read requests. A separate work item table would copy
  that state and drift from it.
- **The engine stays provider-blind.** It compares opaque keys and emails.
  GitHub, Slack and every other source stay in trigger definitions, scripts
  and knowhow, which the team owns and edits. The event matcher stays pure: no
  resolver runs inside matching.
- **Outbound as script triggers keeps it a ramp.** A short question with a
  link leads back into the workspace. A full mirror of every reply would make
  the source the room where the work happens.
- **A soft driver keeps a team moving.** A hard owner blocks work when the
  owner is away. A soft one only routes attention, and an escalation brings in
  the team without taking the work away.
- **An app keeps team policy out of the core UI.** A personal workspace never
  sees a team control, and the team edits its own views.

## Consequences

- A gateway with one operator and no factory app looks and behaves as before.
- `paired-devices.json` migrates once into seed events, all under one
  operator.
- A person whose source email matches no email on their operator reads as a
  stranger until someone adds the email.
- A team that needs person-level permissions, or commits authored by each
  person, is not served. The team setup knowhow says so.
- Intake from people outside the team is unsafe until a sandbox per thread
  exists, since every thread runs with the factory's credentials.
- Offboarding is removing an operator, which revokes its devices, plus
  rotating the factory's bot credentials.
- Read state for notifications and read requests becomes per operator on a
  gateway with two or more people. A read writes a device id, and the person
  resolves at read time. The team's shared flag stays: reads from a gateway
  with one person, and from callers no person holds, still use it.
- Each config event records what its inverse needs: the removed repository's
  fields, a trigger's previous config, a deleted credential's metadata, a
  tool set's previous value. Undo is a config change of its own, so it can be
  undone too. A credential value change, and an MCP server re-registration or
  removal, cannot be undone: their old secrets never reached an event.
- A trigger's actor field travels in its events under `actor_field`, since
  the event bus writes the event's own actor under `actor`.

## Alternatives considered

- **Roles and per-person authorisation.** Rejected: it contradicts ADR 0014,
  and a paired device already reaches a shell.
- **Operators per workspace, in Postgres.** Built first, then rejected: it
  repeats a per-gateway fact in every workspace, and auth cannot depend on
  Postgres.
- **OAuth sign-in per provider to link accounts.** Rejected: email match does
  the job with no provider code.
- **A resolver inside the event matcher** (map a Slack id to a person during
  matching). Rejected: it makes matching impure and provider-aware. A script
  trigger translates instead.
- **A work item table with a lifecycle and source metadata.** Rejected: it
  duplicates thread state, and it would make the engine learn each source's
  vocabulary.
- **Provider-aware routing** (parse a GitHub issue URL, a Slack `thread_ts`).
  Rejected: it couples the engine to providers. A key spec of payload paths
  does the same job with no provider names.
- **Engine code that posts to sources.** Rejected: it couples the engine to
  providers, and a full mirror is a room.
- **Team views in the core drawer.** Rejected: a personal workspace would carry
  team controls. A factory app does it with no core change.
- **Config history tables for undo.** Rejected: the original event already
  records what the inverse needs.
- **Run as the requester** (each person's own credentials). Deferred: only
  needed when a company requires human commit authors and a human PR approver
  does not satisfy that.
- **Pull a team thread down into a personal workspace.** Rejected: the factory's
  scorers must see team work, and steering already works by opening the team
  thread.
- **A sandbox per thread.** Deferred until intake opens to outsiders,
  concurrency outgrows one host, or threads need different secrets.
