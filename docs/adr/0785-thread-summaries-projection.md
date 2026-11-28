# 0785: The `thread_summaries` projection is one exhaustive match over every `ThreadEvent`

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

`EventBus::update_thread_projection` maintains the `thread_summaries`
projection. It runs inside the caller's transaction.

The code needs a rule for how events map to summary columns. It also needs a
rule for which values never become columns.

This record restates the design comments of
`crates/lucidos-core/src/engine/event_bus_projection_thread.rs`, written before
the comment strip removes them. Nobody checked it line by line against the
code.

## Decision

- The projection is one `match` over every `ThreadEvent` variant. Each arm
  updates the metadata for its event.
- A shared second step validates the section transition through the lifecycle
  contract. It then propagates the blocking and attention deltas to ancestors.
- The `match` is exhaustive and never uses `_ =>`.
- The function returns side-effect events to emit after the main transaction
  commits.
- It also returns the ancestor thread IDs whose `blocking_descendant_count`
  changed. The list is empty when the event did not flip the emitting thread's
  `is_blocking` predicate.
- `ActorMode` maps to the legacy two-state `initiator` column. Human becomes
  `"user"`, and Agent and Engine become `"system"`.
- `SessionStarted` supplies the `repo_id` and coding-agent kind fields.
  `ContinuationStarted` supplies none of them.
- `app_id` from `SessionStarted` is not stored. It is the last segment of
  `coding_agent_folder`, so consumers reconstruct it.
- A form request changes no status or section.
- Thread users and the agent pause have no summary column.

## Rationale

- The arms share the `extra_ancestors`, `skip_blocking_propagate` and
  `prev_sample` locals. Splitting them would mean threading that mutable state
  through helpers, which is a logic change.
- Upsert events must create the row before the contract checks it. That is why
  the metadata step runs first and the transition step second.
- An exhaustive `match` forces a decision about the projection whenever a new
  `ThreadEvent` variant is added.
- The caller rebroadcasts each changed ancestor aggregate over SSE. Without it,
  the frontend's `meta.blockingDescendantCount` stays stale until a full
  `/api/v1/threads` reload.
- A stale count breaks live UI gates. One example is the Archive button, which
  hides while a CC sub-thread is running.
- The `initiator` column was never tri-state.
- `ContinuationStarted` only marks a fresh CC subprocess on an existing thread.
  The first `SessionStarted` already locked in the kind and folder.
- A form request ends the turn that asked. The pending list and the transcript
  row are how the user finds an open one.
- Thread users resolve at read time through `engine::thread_users`. The pause
  folds from events at read time through `engine::agent_pause`.

## Consequences

- The projection stays one large unit.
- A new event variant fails to compile until someone decides what it projects.
- The frontend sees live ancestor blocking counts without a reload.
- `initiator` stays two-state. `LegacyInitiator` in `core/store/threads.rs` is
  the matching read path.
- Consumers that need the app id rebuild it from `coding_agent_folder`.
  `change_ops::load_apply_kind_context` is the reference.
- Users and the pause are read from their own modules, never from
  `thread_summaries`.

## Alternatives considered

- Splitting the arms into extracted helpers. This would require threading
  shared mutable state, which is a logic change.
- Promoting `initiator` to a tri-state column. This would need a migration and
  a frontend type change.
- Storing `app_id` as its own column. It is already derivable from
  `coding_agent_folder`.
- A `_ =>` catch-all arm. It would hide new variants.
- Summary columns for thread users or the agent pause. Both are resolved at
  read time.
