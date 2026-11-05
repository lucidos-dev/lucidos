# 0589: While gateways are paired, a Lucidos-source change lands only once origin/main has it; the lane fetches, merges onto origin/main and pushes before it settles

- **Status**: Accepted
- **Date**: 2026-10-27
- **Amends**: [0463](0463-landing-lane.md)

## Context

Remote gateways (design `docs/plans/2026-10-26-remote-gateways-design-in-progress.md`,
points D7 to D10) let more than one gateway land Lucidos source. Each gateway
has its own checkout and its own landing lane. Until now the lane merged onto
the local `main`, moved it, and pushed in the background. A rejected push only
wrote a log line. With two writers, local `main` and `origin/main` would then
drift apart, and a change shown as applied would exist on one machine only.

## Decision

This holds when the lane repository has an `origin` remote and the engine's
gateway holds a gateway pairing. Then `origin/main` is the authority, and a
change has landed only once `origin/main` holds its landing merge. This is
*origin-first landing*.

- **A pass fetches first.** Under the merge lock, the lane fetches `main` from
  `origin` and builds its landing chain onto the newer of local `main` and
  `origin/main`. Local `main` that `origin` lacks is pushed with the chain.
- **It pushes before it settles.** The push runs under the merge lock. Only
  after it succeeds does local `main` fast-forward and the members announce
  `ChangeApplied`.
- **A rejected push re-runs the pass.** The lane fetches again and rebuilds
  the chain on the new tip, a few times at most. A member that now conflicts
  is set aside into the lane batch, like any conflict.
- **A batch catches up with `origin/main`.** Landing a batch fetches, merges
  the fetched tip into the integration branch and pushes. A conflict goes
  back to the lane thread as a catch-up turn.
- **An unreachable `origin` leaves the members queued.** The lane snapshot
  carries the reason (`waiting_for_origin`), and the client shows it. The lane
  keeps no clock (ADR 0463). It tries again on its next message, and on
  `OriginAnswered`, which the task that follows `origin/main` sends whenever a
  fetch gets through. A diverged local `main` waits the same way, since merging
  it would create a merge nobody owns.
- **Every other push is loud.** The Apply paths outside the lane still push
  in the background. A failed push emits `MainPushFailed`, and the client
  shows it.

- **Every gateway follows `origin/main`.** A source checkout's engine runs
  `git ls-remote` every `follow_main_interval_minutes` and fetches only when
  the tip is new. Under the main locks it fast-forwards local `main`, then
  joins or starts a background build. "Switch to new version" still waits for
  a tap, so a pulled version never restarts the engine by itself.
- **A follow never discards and never merges.** A checkout on `main` moves by
  `merge --ff-only`, so git keeps an uncommitted edit or refuses when the new
  commits touch it. A checkout on another branch keeps its HEAD and tree, and
  only the ref moves. A refused follow emits `MainFollowRefused` once per
  reason, and the client shows it.

A repository without `origin` lands locally, as before: it has no second
writer.

### Only while a gateway pairing exists (amended after review)

A second writer exists only while gateways are paired. So origin-first landing
and following `origin/main` run only then.

- **Paired means either direction.** The gateway added a remote gateway, or
  another gateway redeemed this one's pairing code.
- **The engine asks its gateway on every pass and every follow round**, over
  the loopback control route `GET /~/api/v1/control/gateway-pairings`, which
  answers `{ "paired": bool }`.
- **A packaged install is never origin-first.** It follows releases rather
  than `origin`, so no follow round would retry a landing that waited.
- **No gateway at all means unpaired.** That is an engine started without
  `LUCIDOS_GATEWAY_PORT`. A gateway that predates the route answers 404, and
  that reads as unpaired too.
- **A gateway that does not answer means wait**, as an unreachable `origin`
  does. A landing never guesses its way past an unknown.
- **Unpaired lands locally, then pushes in the background.** A failed push
  emits `MainPushFailed`, as before this ADR. The follow task neither polls
  nor reports a refusal.

This keeps a checkout that cannot push landing as it did before: an
`install.sh --dev` clone of the public repository, or a `main` that requires
pull requests.

## Rationale

The point of a shared `main` is that every gateway can follow it by
fast-forward. That holds only if no gateway ever publishes a merge locally
that `origin` refused. Pushing before settling gives "applied" one meaning on
every machine. It also turns a race between two lanes into an ordinary
rejected push, which the next pass resolves.

## Consequences

- A paired landing now needs the network. A paired gateway offline from
  `origin` keeps changes queued instead of landing them, and says so.
- A pass holds the merge lock across a fetch and a push. Other movers of
  `main`, such as a revert or the clean-repo commit, wait a little longer.
- Recovery after a crash between the push and the local move searches
  `origin/main` as well as `main` for a landing merge.

## Alternatives considered

- **Land locally, push after, retry on the next Apply** (the old behaviour).
  Rejected: with two writers it diverges silently, which is the failure this
  ADR exists to stop.
- **Rebase onto `origin/main` on rejection.** Rejected: the lane builds merges
  with `merge-tree`, and a rebase would rewrite the members' commits. A fresh
  pass over the new tip gets the same result without rewriting anything.
- **Merge a diverged local `main` into `origin/main` automatically.**
  Rejected: the merge would have no owner and could carry conflict
  resolutions nobody reviewed. The lane waits and says why.
