# 0086: The file listing leaves the chat prompt: files are discovered on demand, and the freed budget goes to knowhow routing

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

The *workspace payload* (`engine/chat/process/workspace_payload.rs`) is three
sections in one budget: the `[CURRENT FILES]` listing, the Available Apps list,
and the knowhow routing list. `WORKSPACE_PAYLOAD_BUDGET_CHARS = 12_800` caps all
three together, enforced by `busy_workspace_payload_stays_under_budget`.

The listing is already a sample rather than an inventory: `MAX_DIRS = 40`,
`MAX_FILES_PER_DIR = 8`, `MAX_FILES_TOTAL = 120`. The comment on the
per-directory cap says eight files is "what KIND of directory it is, which is
all a routing signal needs". Its one job is resolving a phrase like "the report"
to a path without a tool call.

The three sections look alike and are not. They differ in whether the agent has
another way to get the same thing.

## Decision

**Remove the `[CURRENT FILES]` listing from the chat prompt.** Files are
discovered on demand through `list_files`, `glob_files` and `grep_files`.

The Available Apps list and the knowhow routing list stay. The budget constant
stays where it is, so the headroom the listing used goes to those two, and in
practice to routing.

This is unconditional. It is not gated on ADR 0085's experimental context mode,
and applies whether that flag is on or off.

## Rationale

**The three sections have three different discovery stories, and only one of
them is load-bearing.**

- **Knowhow routing cannot go.** `load_knowhow` takes an id and nothing lists
  them, so the routing list is the only way the agent learns a recipe exists.
  The descriptions are also the retrieval index, since matching happens against
  that text.
- **Apps sit in the middle.** `list_apps` exists, so discovery is possible. But
  every named app must be linked as `[Habit Tracker](app:habit-tracker)`.
  Dropping the list would put a tool call in front of almost any reply that
  mentions one.
- **Files have three replacements**, and all three walk everything rather than a
  truncated sample.

**The sample fails exactly where it would be needed.** 120 files out of a large
workspace is a lottery ticket for path resolution. The agent falls back to
`glob_files` anyway and has paid for the listing twice. On a small workspace the
120 might be everything, and there the tool call was cheap to begin with. The
value runs inversely to workspace size.

**Removing it buys routing quality, not just tokens.** One budget covers all
three sections, so the listing was competing for space with the only section
that has no alternative. This is a reallocation more than a cut.

**It is also a listing of a moving target.** Files change constantly, including
while a turn is running. So it is stale by construction in a way an app
inventory and a knowhow set are not.

## Consequences

- **The agent pays a tool call when it needs a path it cannot guess.** That is
  the intended trade, and `glob_files` answers better than the sample did.
- **The knowhow routing list gets the headroom.** Whether that becomes more docs
  or longer descriptions is a separate call, bounded by
  `KNOWHOW_DESCRIPTION_MAX_CHARS = 400`.
- **`busy_workspace_payload_stays_under_budget` needs updating**, since it sums
  three sections and will sum two. Keep the constant: the space is reallocated,
  not reclaimed.
- **The `File List` row leaves the LLM Context Viewer**, since
  `build_capture_sections` will no longer be handed one.
- **A small workspace loses a real convenience.** Where the listing was complete
  it saved a round trip on the common "open the report" request.
- **It slightly weakens ADR 0085's control arm.** An eval comparing lean against
  today compares against a today that just changed, so the two should not land
  in the same measurement window.

## Alternatives considered

**Keep it as it is.** Rejected. It costs a share of a budget the routing list
needs more. It is also the one section of the three with a full set of
replacements.

**Shrink it further instead of removing it.** Rejected. It is already at 40
directories and 8 files each, and shrinking a sample makes the lottery worse
rather than cheaper. There is no size at which a sample of a large tree resolves
a specific path reliably.

**Send it on the thread's first call only.** Rejected. That is ADR 0085's shape
applied to the wrong section. Files change constantly, so a first-call listing
is stale within minutes and the agent has no signal that it went stale. A tool
call is correct at the moment it is made.

**Replace it with a directory-only map, no filenames.** Rejected for now, and
the closest thing to a survivor. It would keep the shape signal at a fraction of
the size. It also duplicates the workspace taxonomy, which the system prompt and
knowhow already state, so it is likely to be paying twice for one fact.
