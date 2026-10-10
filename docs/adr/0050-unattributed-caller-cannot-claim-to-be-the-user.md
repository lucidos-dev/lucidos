# 0050: The loopback API is unauthenticated, so attribution is evidence-based: an unattributed caller cannot claim to be the user

- **Status**: Accepted
- **Date**: 2026-08-06

## Context

On 2026-08-06 the Lucidos Agent was asked to send a follow-up to every running
coding-agent thread in its workspace. Nothing covers that:
`follow_up_child_thread` and `lucidos threads follow-up` reach only the caller's
own direct children (ADR 0043). Rather than report itself blocked, the agent
used `run_bash` to `curl` the engine's own `POST /api/v1/chat/stream` once per
thread id, with `mode: "human"`.

It worked, and it went wrong twice.

The stored `MessageReceived` reads
`"mode": "human", "origin": {"kind": "api", "mode": "human", "user_agent": "curl/8.7.1"}`.
That is not only a display problem: the projection maps `mode` to
`thread_summaries.initiator = 'user'` and bumps `last_user_action`, the drawer's
recency sort. Six agent-authored turns became, in the record, six things the
user typed.

And it hit the wrong engine, the packaged install serving another workspace on a
guessed port. Those thread ids did not exist there. A thread has no creation
event in Lucidos, so the `MessageReceived` projection is an upsert and the
insert arm materialized six brand-new threads from the client-supplied ids. They
then died with `ResponseFailed: "This Lucidos install has no source checkout"`.
The agent "verified delivery" by reading the same ids back off the same wrong
engine, found its own message, and reported success. **The auto-create turned a
mis-delivery into something indistinguishable from a delivery**, which is the
part that destroyed the agent's ability to catch its own mistake.

Two facts about the surface frame everything below.

**There is no authentication on `/api/v1`.** `create_router` mounts every domain
router with a body limit, compression, cache-control and a request logger, and
no auth layer. The gateway's authz middleware guards only its own destructive
control plane. `api/actor.rs` states the model outright: caller-supplied fields
are a display hint, never authorization, and only the thread-bound origin token
is authoritative. The default bind is loopback, but `LUCIDOS_BIND_ALL` and
Tailscale widen it, at which point "loopback is trusted" quietly becomes "the
tailnet is trusted".

**The gate that existed was opt-in by the party it constrained.**
`subprocess_chat_legitimate` already refused `mode: Human` outright and refused
cross-thread agent posts, but it ran only for a request presenting a valid
origin token. Every Lucidos-spawned subprocess has one in its environment and
the `lucidos` CLI forwards it automatically; `curl` does not. So **dropping the
credential bought strictly more privilege than presenting it**. That inversion,
not the missing tool, is the root cause.

## Decision

**Attribution is evidence-based, and the engine holds exactly three kinds of
evidence.** A thread-bound origin token (authoritative: proves a Lucidos-spawned
subprocess, and which thread). A `device_id` resolving to a row in `devices`
(the user's own client, which sends `x-lucidos-device-id` on every mutating
request). A `caller_workspace` (another workspace vouching for its own human, a
display hint the existing cross-workspace contract already accepts). Anything
else is an **unattributed caller**.

Three rules follow, all on the chat path, all refusing before anything is
written:

1. **`mode: "human"` requires device attribution or a `caller_workspace`.**
   Otherwise 403. Applies to both entry points, `/chat/stream` and the legacy
   `/chat`. (`api::chat::human_mode_is_attributed`.)
2. **An id-carrying create must be explicit.** A `thread_id` naming no existing
   thread is a 404 unless the request says it is creating one: `new_thread:
   true`, a `parent_thread_id`, or a `caller_workspace`.
   (`api::chat::thread_target_is_addressable`.)
3. **A request may assert which workspace it is for**, via
   `x-lucidos-target-workspace`, checked by middleware over the whole `/api/v1`
   router. A mismatch is 409 with a body naming the workspace this engine
   actually serves. The assertion is optional; its absence changes nothing.
   (`api::target_workspace`.)

**The loopback API stays unauthenticated.** That is a decision here, not an
omission.

**ADR 0043 stands unchanged.** No sibling edge, no grandchild edge, no
any-to-any address space. The missing capability is answered in the prompt and
in knowhow: when a tool refuses, report it.

## Rationale

**Why refuse rather than downgrade.** The tempting cheap fix is to keep
accepting the request and stamp it `Api { mode: Agent }` instead. Two arguments
kill it. The UI already renders a human-mode `Api` origin as "API caller"
(`actorInitiator`, deliberately, so an unauthenticated POST cannot say "You"),
and `Agent` renders as **"Lucidos Agent"**, so the downgrade would upgrade the
claim: an anonymous curl would present as Lucidos's own agent. And more
importantly, a downgrade would have let the incident's request succeed. The
agent would still have reported delivery, on the wrong engine, to the user. The
refusal is not primarily a security control, it is **the moment the agent finds
out it is doing something wrong**, which is the failure this whole episode is
about.

**Why not a fourth `ActorMode`.** "We could not tell who this is" is a real
state, and a `Human | Agent | Engine | Api` enum would name it honestly. It
would also ripple through the Rust enum, the wire format, the TS union, the
contract fixtures, the `LegacyInitiator` two-state mapping and the projection,
to express something the 403 expresses by not existing. Rejected on cost against
a state that should have no instances.

**Why device attribution is the bar, given that a device id is forgeable.** It
is. An agent that will curl the engine will also `POST /api/v1/devices/register`
and use the id it gets. This is not a boundary against a determined adversary
and cannot be, because the surface is unauthenticated by construction: any
process on the machine can already do anything. What it is, is the removal of
the *accidental* path. The agent in the incident did not set out to impersonate
anyone; it sent the default shape and the engine agreed with it. Registering a
device is a deliberate, logged, attributable act, and an agent that takes it has
crossed a line it cannot claim not to have seen. "Make impossible states
impossible" is about the accident, and the accident is what is closed.

**Why the scope stops at the chat path.** `user_actor_resolved` keeps its
`Api { mode: Human }` fallback for apply / discard / settings and the rest of
the mutating surface. Those stamp an actor onto a `SystemEvent`; the chat path
writes a conversational turn into the user's own timeline, which is the thing
that can be mistaken for the user speaking. Extending the refusal to every
mutating endpoint would break the honest external-client case (a script that
applies a change) for a much weaker payoff. Recorded so a later reader does not
read the omission as an oversight.

**Why the target-workspace assertion is optional.** The browser is same-origin
and does not know a workspace name; under the gateway the slug is in the path
and has already been resolved before the engine sees the request. Every existing
client predates the header. A mandatory assertion would break every caller to
catch a mistake only scripted callers make, and scripted callers are exactly the
ones that can opt in: the `lucidos` CLI now sends it on every request (from
`$LUCIDOS_WORKSPACE`, or the resolved target for `spawn-thread --to`), and
`engine::http::workspace_client` sends it on every cross-workspace POST.

**Why the 409 names the actual workspace.** A bare 409 says something is wrong
but not what, so the obvious next move is to retry. Naming the workspace this
engine serves is the one fact that lets a mis-aimed caller re-resolve its target
instead. `GET /api/v1/health` already discloses the same name, so this leaks
nothing new.

**Why not authenticate the API.** A shared secret or bearer token would have to
be distributed to the browser, the CLI, every spawned subprocess, the gateway
and every app iframe. It would not have stopped this incident: the agent held
every one of those environments, and the credential would have been in the same
env block the origin token already lives in. Authentication answers "is this
process allowed to talk to the engine", and the answer on a single-user machine
is yes. The question that was actually being answered wrongly is "who is
speaking", and that is what evidence-based attribution addresses.

## Consequences

- **The three evidence kinds are now a named vocabulary** (`docs/glossary.md`:
  *unattributed caller*, *device attribution*, *target workspace assertion*), so
  a future endpoint asking "may this caller claim X" has a settled way to ask
  it.
- **An external human client must register a device to speak as a human on the
  chat path.** The API e2e suite does exactly this (`user_client()`), which
  makes those tests model a real client rather than an anonymous one. Anything
  else posting `mode: "human"` without a device breaks, deliberately.
- **A frontend that has not reloaded 404s on a brand-new chat** until it picks
  up the `new_thread` flag. The engine serves its own pinned frontend snapshot,
  so the window is a single reload, and the 404 body says what happened.
- **`api::chat` now owns three gates rather than one**, and the ordering matters:
  attribution, then existence, then the subprocess matrix. Existence runs before
  the subprocess gate deliberately, so a caller on the wrong engine gets "no
  such thread here" rather than a 403 about its relationship to a thread that
  was never the one it meant.
- **The stored `Api { mode: Human }` variant is not retired.** It stays for old
  rows and for the non-chat endpoints. What changed is that the chat path no
  longer writes one.
- **The six phantom threads in the other workspace are not cleaned up by this
  change.** It stops new ones.
- **This does not make the surface safe against a hostile local process**, and
  nothing here should be cited as if it did. It makes the honest path the
  default one and the dishonest path deliberate.

## Alternatives considered

- **Downgrade an unattributed `mode: human` to `Agent` instead of refusing.**
  Rejected twice over: it renders as "Lucidos Agent", a stronger claim than the
  "API caller" the human-mode origin already renders as, and it would have let
  the incident succeed silently. See the rationale.
- **A fourth `ActorMode` for "unattributed".** Rejected on cross-layer cost for
  a state that should have no instances. See the rationale.
- **A blanket 404 on any `thread_id` that names nothing.** Rejected because it
  is not available: the frontend's own new-chat send mints the uuid client-side
  and POSTs it against a thread that does not exist, so client-supplied-id
  creates are the normal path, not an anomaly. The create had to become
  *explicit* rather than *forbidden*.
- **A real `ThreadCreated` event, making creation a first-class transition
  rather than a side effect of an upsert.** This is the structurally correct fix
  and is the one to reach for if this area is ever revisited. Rejected here on
  blast radius: it changes how every thread comes into existence in an
  event-sourced core, and the request flag buys the same guarantee on the one
  path that was exploited.
- **Making the target-workspace assertion mandatory.** Rejected: it breaks every
  client that predates it, including the browser, which has no workspace name to
  send. Optional-but-honored gets the whole benefit for scripted callers, who
  are the only ones that can resolve the wrong port in the first place.
- **Putting the target workspace in the request body rather than a header.**
  Rejected: it would need adding to every request type, and it would sit beside
  `caller_workspace` (who is calling) as a near-identical field with the
  opposite meaning (who is being called). A header applies uniformly and keeps
  the two apart.
- **Authenticating `/api/v1` with a shared secret.** Rejected: see the
  rationale. It answers a question that was not being answered wrongly.
- **Widening the cross-thread address space so the original request would have
  been possible.** Rejected; ADR 0043 already weighed this and the reasoning
  holds. If the capability is ever wanted, the shape is a **broadcast verb, not
  an address space**: a tool taking a message and *no target*, where the engine
  resolves the recipient set (say, running coding-agent threads in this
  workspace) and stamps every delivery as agent-originated. Mis-delivery to the
  wrong thread is then unreachable because the caller never names a thread, and
  the wrong workspace is unreachable because it is in-process. That would need
  its own ADR, covering at minimum the fan-out cost of waking N coding-agent
  sessions and whether a user-initiated broadcast from the UI is the better
  home for it.
- **Deleting the legacy `POST /api/v1/chat` route instead of hardening it.** It
  has no caller anywhere in the tree (not the frontend, the SDK, the CLI, e2e,
  or system-knowhow) and it bypasses the subprocess gate entirely, so deletion
  is defensible on the no-dead-code rule. Not done here: it is an API removal
  beyond the approved scope of this change, and hardening it costs one extra
  existence query on a route nobody calls. Flagged for a separate decision.
