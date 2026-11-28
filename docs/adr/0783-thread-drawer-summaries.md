# 0783: The drawer loads all inbox threads plus one global `created_at` slice of archived threads

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

The thread drawer shows a REVIEW pile of inbox threads and an Archive pile of
archived threads. `summaries.rs` builds the initial window, the infinite-scroll
pages, the filter facets and the thread families.

The drawer sorts the Archive section by `created_at`, which is also the date
each row shows. The server must window, page and filter on the same terms as
the frontend, or rows go missing or show out of place.

This record restates the design comments of
`crates/lucidos-core/src/core/store/threads/summaries.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- `get_recent_threads` returns every thread with `archive_state='inbox'`. Inbox
  is unbounded.
- It also returns the newest `archive_limit` archived threads, as a single
  global `created_at DESC` slice over archived rows, whatever their source.
- `get_older_threads` pages below a `before` cursor. The cursor is a
  `created_at` timestamp, the oldest loaded thread's `created_at`.
- The channel and facet predicate is `channel_facet_filter_sql`, the exact
  mirror of the frontend `threadPassesChannelFilter`. It also drives
  `count_archived_threads`.
- A `sources` channel gate comes first. Then `trigger_ids` narrow trigger rows,
  and `repo_ids` and `app_ids` narrow coding-agent rows, which are stored as
  `claude_code`. Whole channels and facets compose as a union.
- The `has_response = TRUE` gate is relaxed whenever any facet axis is
  sub-selected.
- The repo facet has no `source` gate, because `cc_repo_id IS NOT NULL` already
  scopes it. It matches the `cc_repo_id = ANY(...)` predicate in
  `get_older_threads`.
- The repo facet groups first, then resolves `name` per distinct id in the
  outer query. It uses the live registry, then the `repo_names` projection.
- A family helper loads every ancestor and descendant, linked by
  `parent_thread_id`, of the base set that is not already in it.
- The family extension is capped by `last_activity`. `max_family` limits the
  result after `ORDER BY last_activity DESC`, and `i64::MAX` opts out.
- The family walk uses `UNION`, not `UNION ALL`. A single walk with an OR join
  climbs to ancestors and descends to children in one pass.

## Rationale

- An inbox row is one the user has not dismissed. Capping it would silently
  hide work the user expects to see, such as crashed coding-agent sessions and
  idle chats.
- Archived threads are not time-sensitive, so a cap is fine. The user can page
  back with `get_older_threads`.
- One global `created_at` slice is a contiguous prefix of the archived list.
  Window, display sort and cursor share one axis and granularity, so the seam
  between the initial window and the first scroll page has no gap.
- Paging and display share one axis. A recently created but stale thread then cannot page in late and vanish from the top of the created-sorted list.
- The outer `ORDER BY` of the recent query is irrelevant, because the frontend
  re-sorts every section.
- The `has_response` gate is relaxed for facets. The dropdown from `get_filter_facets` advertises every trigger, repo and app that ever stamped a row, with no `has_response` gate. Selecting one must show its threads, or
  the dropdown lies.
- Facet and filter must advertise the same set. That is why the repo facet and
  `get_older_threads` both skip a source gate.
- Grouping before resolving `name` keeps the name lookup out of the `GROUP BY`.
  A removed repo lists under its historical name instead of its UUID.
- The family helper exists because `ThreadDrawer.tsx` `nestByParent` renders a
  whole family together. Without it, a member whose own `created_at` falls below the window would vanish. The parent's badge would read "N/N done" while only in-window children render.
- The family cap uses `last_activity` so that an actively streaming sub-thread
  created long ago is still kept.
- `UNION` ends the recursive walk even on a corrupted parent cycle.
- The family cap exists because a pathological fan-out would otherwise pull
  every descendant into the initial `/api/v1/threads` payload. That would
  balloon the frontend `threadMap` and its re-render cost.

## Consequences

- The unfiltered view loses the guarantee that the newest thread of every
  source sits on top. Old coding-agent threads sit at their true date,
  reachable by scroll or the coding-agent channel filter.
- Saved sorts by `last_user_action`, but Saved is fully loaded through
  `get_saved_threads` and is not paged here.
- Facet selections now surface threads that never produced a response, such as
  a crashed coding-agent session or an errored trigger run.
- With no facet axis active, the filter reduces to the plain `has_response =
  TRUE` channel view.
- Under the family cap, the newest members are kept. A visibly old descendant
  below the cap matters only when the user expands the family, and then they
  can scroll-paginate down to it.

## Alternatives considered

- A per-source archive window was rejected. It is a union of per-source
  `created_at` prefixes, which one global cursor cannot page without gaps. With
  dense chats and sparse older coding-agent or trigger threads, it dragged the
  cursor to the sparse source's months-old boundary. That surfaced coding-agent
  threads far above their real date, skipped every chat in between, and halted
  scroll early.
- Capping the inbox was rejected, because it would silently hide undismissed
  work.
- The old two-branch filter form was rejected. Any facet selection silently
  dropped the whole-channel rows.
- Capping the family extension by `created_at` was rejected, because it would
  drop an actively streaming sub-thread created long ago.
- `UNION ALL` for the family walk was rejected, because it would not terminate
  on a corrupted parent cycle.
