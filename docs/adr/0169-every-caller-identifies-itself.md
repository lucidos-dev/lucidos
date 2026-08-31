# 0169: Every caller identifies itself: the mode-human default dies, and four credentials that already exist cover every caller

- **Status**: Accepted
- **Date**: 2026-08-30

## Context

`api::actor::user_actor_resolved` resolves an actor for every mutating handler
that has no parent-thread context. Apply, Discard, answering a question card,
restarting a turn, settings writes. When it finds no device and no origin token,
it returns `Api { mode: Human }`.

That default is an inversion, and ADR 0050 already named the shape of it. A
caller presenting a thread-bound origin token is stamped `Agent`, because
`build_message_origin` checks `subprocess_origin` first and says so in a
comment. A caller that drops the token is stamped human. So dropping the
credential buys more than presenting it.

ADR 0050 closed that on the chat path with `human_mode_is_attributed`. It was
deliberately scoped there, and the glossary records the carve-out: the fallback
stays for apply, discard and settings, which stamp a `SystemEvent` rather than
writing a turn into the user's timeline. ADR 0083's amendment and
`api::thread_reach` both then note that closing the untokened path is owed its
own ADR. This is that ADR.

**The default is not mainly letting curl in. It is covering for Lucidos.** In 60
days on a live workspace, 9170 api-actor events carried a token and said agent.
260 said human with no evidence behind the claim. They split into two
populations, and neither is a stranger.

**196 are the engine talking to itself.** Every one carries the label `engine
build`, which comes from `run_engine_cargo_build` in `scripts/lib/workspace.sh`.
Its own comment names the caller: the engine's background rebuild, reaching
there via `web-dev.sh --engine-build`. It takes a *build slot* through the CLI,
with no origin token, because a background rebuild belongs to no thread.

**64 are the owner's own browser.** `BlogPreviewRequested`, `TriggerUpdated` and
`SitePublishRequested`, with a Chrome or mobile Safari user agent. The browser
holds a device id and sends it on most mutating fetches. Those three routes do
not.

## Decision

**Every caller to the engine identifies itself, and the `Api { mode: Human }`
fallback is removed.** A caller that presents no identity is refused rather than
recorded as the user.

Four credentials cover every caller, and all four already exist. Nothing new is
minted.

| Caller | Identity it presents |
|---|---|
| The engine's own build-watch and release scripts | the machine-local token `api::local_auth` already mints |
| A thread's subprocess | the *thread-bound origin token* it already carries |
| A browser on the loopback port | the *device attribution* it already holds |
| A client through the *workspace gateway* | the paired device the gateway already injects |

**Teach the four, then refuse.** The build-watch sends the machine token, and
the three frontend routes send the device id. The default dies after that, in
the same plan. Flipping first would 403 the engine's own rebuild at the build
slot, which is the collision ADR 0070 exists to prevent.

**This is attribution, not authentication, and it changes neither door.**
Loopback stays unauthenticated, exactly as `api::local_auth` decided: reaching
the socket proves the caller is a process on this machine. A wide bind stays
locked behind the machine-local token. The gateway keeps pairing its clients.

## Rationale

**On loopback there are no strangers, so the gate buys honesty rather than
safety.** `api::local_auth` already separates two questions.
`api::browser_origin` asks which document sent this, and `local_auth` asks who
is calling. Neither asks on whose behalf the caller acts. The human default is
what fills that third silence, and it fills it with a guess.

**A default that names the user is worse than no answer.** It made two ordinary
bugs invisible for months. A frontend route that forgets its device id looks
identical to the owner clicking. A script that loses its token looks identical
to the owner running it. Refusing turns both into a failure someone fixes.

**The measured cost is small and already known.** Three callers need teaching,
and each has a credential waiting. Everything else in the workspace already
identifies itself, which is what the 9170 tokened events show.

**ADR 0168's clause 4 depends on this.** That clause says a verb wider than a
thread's own subtree needs evidence of the person. Evidence that can be
fabricated by omitting a header is not evidence. Until this lands, clause 4 is a
convention.

## Consequences

**What we keep.**

- The 32 applies from a bare curl become refusals. So do the handful of question
  cards answered the same way, rather than landing as entries attributed to the
  owner.
- The e2e suites keep working on loopback, because they are processes on this
  machine and can read the machine-local token.
- Every existing tokened caller is unaffected. It already identifies itself.

**What we give up, knowingly.**

- **Three callers must be taught before the flip.** Until then the inversion
  stands, and the plan carries both halves so the second cannot be forgotten.
- **A machine-local token names a machine, not a person.** Two scripts
  presenting it are one identity in the log. That is a real loss of resolution,
  accepted because the alternative is registering a device per script.

**What changes elsewhere.**

- `user_actor_resolved` loses its fallback, and every caller of it inherits the
  refusal.
- `docs/glossary.md` § *unattributed caller* loses the carve-out sentence naming
  apply, discard and settings.
- ADR 0083's amendment records this residual as open. It is closed here.

## Alternatives considered

- **Refuse everywhere immediately.** Rejected on order rather than on substance,
  and it is the same end state. It 403s the engine's own rebuild at the build
  slot, so two cargo builds could collide while the caller is being taught.
- **Record an honest unidentified actor and refuse later.** Rejected as a
  half-step that leaves the flip to a follow-through nobody owns. Teaching three
  callers is small enough to do in one plan.
- **Refuse only the root verbs from ADR 0168, and leave the rest defaulting.**
  Rejected. It closes the dangerous half first, which is attractive. But it
  leaves a default that lies on every other route, and keeps the frontend gaps
  hidden.
- **Register a device per script.** Rejected as more machinery than the problem
  needs. The machine-local token already exists for exactly this class of
  caller, and a build-watch is not a person.
- **Authenticate the loopback port.** Rejected, and out of scope. Reaching the
  socket already proves a local process. The frontend and both e2e suites depend
  on that. `api::local_auth`'s own header explains why locking it would break
  them for nothing.
