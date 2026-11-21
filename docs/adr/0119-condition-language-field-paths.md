# 0119: A condition key is a field path, exact key first, with $nin, $regex and $or; no array indexing

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

An *event subscription*'s `condition` resolved each key with a flat one-level
lookup, so a subscriber could filter only on top-level payload fields.

The case that forced it: the GitHub `workflow_run` webhook emits
`GithubWorkflowRunStateChanged`, whose payload carries `action` at the top and
`event` / `conclusion` under `workflow_run`. The "Scheduled CI result" trigger
could cut only on `action`. Its script re-implemented the rest of the filter in
Python, after the engine had already spawned a trigger thread.

Two properties bound every option. Conditions are persisted data, in
`TriggerCreated` / `TriggerUpdated` payloads and in armed `EventWaitStarted`
rows, so nothing may be migrated. And one predicate serves both subscription
species, so whatever changes has to change for both at once.

## Decision

A condition key is a **field path**: `workflow_run.event` reads one level down.
`$nin`, `$regex` and `$or` join the operator set, and a malformed condition is
refused at the write surface.

Seven rules, each pinned by a test:

1. **The exact key wins, at every level.** Resolution tries the whole remaining
   key as an object key first, and only then splits at the first dot.
2. **A path that resolves to nothing is JSON null**, exactly as a missing
   top-level key always was.
3. **A numeric segment is an ordinary object key.** An array anywhere on the
   path ends resolution, and there is no index syntax.
4. **`$nin`** is the complement of `$in`, and a malformed list fails closed on
   both.
5. **`$regex`** is an unanchored search over a JSON string only, with case set
   by the inline `(?i)` flag, compiled per evaluation.
6. **`$or`** sits in key position, takes a list of whole conditions, ANDs with
   its siblings, and nests at most 8 deep.
7. **`condition::validate`** runs inside the one subscription check, which is
   renamed `check_subscriptions` because its scope grew past event types.

## Rationale

**Exact-key-first makes backward compatibility structural rather than a claim.**
The first lookup the resolver performs at the top level is precisely the lookup
the flat evaluator always performed. Traversal therefore runs only where the old
code already resolved to absent, so no stored condition can change verdict. That
is a mechanical property, not a promise to be careful. It also keeps a
third-party payload's literal dotted key nameable, which traversal-first would
make impossible without an escape syntax.

**Keeping missing-means-null was the compatible choice, and it happens to be the
coherent one.** `{"conclusion": {"$ne": "success"}}` matches an event with no
`conclusion` today, so any other rule would move a stored condition's verdict.
One rule for one level and for many also leaves the language without a seam. The
corollary is worth having on its own: `{"x": {"$ne": null}}` already reads as "x
exists and is not null", so no `$exists` operator is needed.

**Refusing arrays is the only decision that stays reversible.** A numeric segment
against an array resolves to null today, so nothing stored can depend on a
positive result. Turning indexing on later could therefore only turn non-matches
into matches, and the reverse would not hold. Positional access is also usually
the wrong predicate: what a subscriber wants from `commits` is "any element
matches", which is a quantifier and a separate decision.

**Rust's regex crate is what makes `$regex` affordable.** It is already an engine
dependency, and it is linear-time with no backtracking, so a user-authored
pattern carries no ReDoS exposure. Without that property this would be a
security decision rather than an ergonomics one.

**Write-time refusal is the module's existing principle, extended.** An
unsupported operator evaluates to false and says nothing, so a subscription
carrying one arms clean and waits forever. `$regex` and `$or` each add a new way
to hit that: an unparseable pattern, and a malformed branch list. The
subscription check already refuses a misspelled event name synchronously for the
same reason, so the condition joins it there.

## Consequences

**A `$`-prefixed key in field position is reserved.** It is refused at write
time, and `evaluate` returns false for one rather than resolving it against the
payload. Both halves are needed. Refusal alone would leave a payload carrying a
literal `$and` key able to satisfy a stored condition naming it. Promoting
`$and` to a combinator later would then change that verdict.

The cost is that a payload field whose name starts with `$` is unnameable, which
reaches `$schema` and `$ref` as well as `$or`. That is accepted: such a field is
rare in an event payload, and the reservation is what lets a later combinator
land without auditing every install's stored conditions.

**An older engine reading a new condition under-fires.** A dotted key resolves to
absent there, and `$regex` / `$or` read as unknown operators, which are false. It
never over-fires and it never errors. So a condition using the new syntax must
not be written until the engine carrying this change is running.

**Adding an operator revives a condition that named it.** Such a condition was
inert, because an unsupported operator evaluates to false. So a stored
subscription carrying `$nin`, `$regex` or `$or` starts matching where it
previously never did. That is inherent in adding an operator rather than
avoidable, and it is why the set was kept small and deliberate. The dev
workspace's store was checked before landing this: `$in` is the only operator any
stored condition uses.

**Validation is not retroactive.** It runs at the write surface only, so a
condition already in the event store is never re-validated and its verdict cannot
change. One consequence is visible: `{"workflow_run": {"event": "schedule"}}` has
always matched nothing, and is now refused at write with a message naming
`workflow_run.event`.

**Two silent footguns close.** A filter on the command text inside `args` is now
expressible, as `{"args.command": {"$regex": "…"}}`. And a trigger no longer
needs a script to finish a filter the engine could not state.

## Alternatives considered

**Path traversal wins over a literal dotted key.** Rejected. Backward
compatibility would stop being structural and become a claim resting on no
stored condition naming such a key. It also leaves a real webhook field
unnameable, with an escape syntax as the only way back.

**Longest-prefix resolution**, trying `a.b` as a head before `a`. Rejected as
ambiguous and quadratic for no gain. "The whole remaining key, or a left-to-right
walk" is one sentence a reader can hold. The exact-key check at each level
already covers the case that matters.

**An escape syntax** (`a\.b`) or an array-of-segments key form. Rejected.
Exact-key-first already makes a dotted key nameable, so this would be a second
mechanism for a problem that no longer exists.

**A missing path that never matches**, instead of null. Rejected: it changes the
verdict of stored `$ne` conditions, which the compatibility invariant forbids.

**Array indexing by numeric segment.** Rejected for now. It gives one segment
spelling two meanings, since a numeric key on an object already resolves
literally. It also answers a question subscribers rarely have. A quantifier
(`$any` / `$elemMatch`) is the real feature and deserves its own decision.

**`$exists`, `$and`, `$not`, `$nor`.** Rejected. `{"$ne": null}` is `$exists`,
several keys in one object are `$and`, and `$ne` / `$nin` cover the negations
that come up.

**A compiled-regex cache.** Rejected for now. `evaluate` stays a pure function of
`(condition, payload)`, and a global mutable cache inside a predicate is real
design cost against a compile measured in microseconds. Revisit if a profile ever
shows regex compilation inside the trigger fan-out, which needs both many
subscribers carrying patterns and a high-rate event class.

**A second, richer predicate beside the existing one.** Rejected outright. One
predicate for both subscription species is the invariant
`core::event_subscription` exists to hold, and its module doc says why.
