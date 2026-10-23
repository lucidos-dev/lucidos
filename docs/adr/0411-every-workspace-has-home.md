# 0411: Every workspace has Home: the experimental switch is removed, the welcome lives in Home, an unused Home costs nothing, and Home's follow-up reach is accepted

- **Status**: Accepted
- **Date**: 2026-10-10

Plan: `docs/plans/2026-10-10-home-on-by-default-and-welcome-in-home.md`.
Amends [ADR 0362](0362-tree-memory-module-and-the-home-thread.md), whose
consequence put Home behind the `home_thread_enabled` switch.

## Context

v0.47.0 shipped the home thread off, behind an experimental switch, while it
proved itself. The maintainer's goal is that a new install starts in Home. On
the first draft of the plan he decided: "we drop the switch, errybody get Home
but can choose to never use it".

Two facts shaped the rest. Since ADR 0381, a model call no thread made records
on Home, so most workspaces already held a hidden Home. And the first-run
welcome was never a message: the app drew it on the empty compose view.

## Decision

**Every workspace has Home, always.** Boot creates it before the router
serves. `home_thread_enabled` is gone from the catalog, the gate, the list
filter, the voice refusal and Settings. A migration deletes stored rows,
`false` included. Home keeps its id and history, and shows. A write of the
retired key is refused (`RETIRED_KEYS`).

**The welcome lives in Home.** It stays a client surface. It renders in Home
when Home exists, and on the compose view only when Home could not be made.
An empty Home showing it takes the compose layout, its title at the top. The
setup interview sends into Home.

**A device with no stored focus opens Home.** A returning user keeps the
thread they had open.

**Home's follow-up reach is accepted as it is**, for every user.

## Rationale

**Never using Home costs nothing**, which is what makes "can choose to never
use it" true without a switch:

- A Home turn starts only from the user writing there, a call (voice stays off
  by default), or a report from a sub-thread Home spawned. Nothing else can
  post into it.
- The summary tree reads messages, replies and tool calls. Cost rows are none
  of them, so an unused Home's tree is empty and the compactor never calls a
  model for it.
- Cost rows move nothing in Home's summary row, so it never reads as new.
- Its reach and its owner-button presses act only inside a Home turn.

`an_unused_home_costs_nothing` pins the second and third facts.

**The welcome cannot be a message.** A stored welcome would freeze its
provider variant at creation, though a provider can be added later. It would
also enter Home's summary tree and memory, and pose as an agent reply no model
wrote.

**The follow-up widening is bounded.** A Home turn can message any thread.
Text Home read elsewhere could steer it into a follow-up to a coding-agent
thread, which then acts with the grants its own session holds. Owner buttons,
Apply included, still need the owner's words or approval, and the two widest
Always-allow grants are never an agent's. So the harm stops at what that coding
agent may already do unasked, and any parent already has this reach over its
own children.

## Consequences

- One `thread_summaries` row per workspace that never goes away. Home cannot be
  archived or deleted, and no setting hides it.
- A user who explicitly turned Home off sees it again. One release notice tells
  existing users what Home is. A fresh workspace skips it, as it skips every
  notice.
- A returning user whose last screen was the empty compose view lands on Home,
  since that view stores no focus.
- The browser e2e helper `navigateToApp` steps off Home, because most specs
  start from the compose view.

## Alternatives considered

- **Flip the default and keep the switch.** The first draft's recommendation:
  it honoured an explicit `false` and left a way out. Lost to the maintainer's
  decision. Without the switch, the gate and its filter go too.
- **A migration that sets every workspace to `true`.** Lost: it keeps a
  switch nobody needs, and overrides the people who chose `false` anyway.
- **Store the welcome as Home's first message.** Lost, see Rationale.
- **The welcome as the first card above a docked prompt.** Rendered beside the
  chosen layout. Lost: the compose layout matches the first run every user has
  seen, and the code already existed.
- **Limit Home's follow-up to chat threads.** Lost: it cuts Home's main use,
  following up coding work, to close a path that already needs a misled agent.
