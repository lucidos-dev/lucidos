# 0808: Custom folders: the folder list is a preference, membership is a thread event

- **Status**: Accepted
- **Date**: 2026-11-24

## Context

Users grouped pinned threads with title prefixes, because Pinned had become
their "active work" list. They asked for named folders as top-level drawer
sections. A custom folder needs two pieces of state: the list of folders (name
and order), and which folder each pinned thread is in.

Plan: `docs/plans/2026-11-24-custom-folders-for-pinned-threads.md`.

## Decision

The folder list is the workspace preference `thread_folders`, a validated JSON
array of `{id, name}` in display order. Membership is the thread event
`ThreadMovedToFolder { folder: Option<String> }`, projected to
`thread_summaries.pin_folder`. The engine keeps `DisplaySection::Saved`, and the
client splits saved families into folders by the root's `pin_folder`.

## Rationale

- **The list is user data, not lifecycle.** Names and order are a setting, like
  `response_styles`. A preference already syncs to every device, takes agent
  writes, and validates as a whole array. A table would add a store, an API and
  SSE plumbing for twenty short strings.
- **Membership belongs to the thread.** Where a thread sits is part of its
  history, next to `ThreadSaved`. An event replays, reaches every client over
  the existing aggregate, and lets the pin rules clear it in one place: unsave,
  archive and discard all set `pin_folder` to NULL.
- **No stored event changes shape.** `ThreadSaved` stays a unit variant. A pin
  into a folder emits `ThreadSaved` then `ThreadMovedToFolder`. Old workspaces
  replay unchanged, and NULL means Pinned.
- **A CHECK keeps a folder pinned.** `pin_folder IS NULL OR is_saved` makes "in
  a folder but not pinned" impossible in the database, not only in the code.
- **Deleting a folder needs no thread writes.** A `pin_folder` that names no
  listed folder renders in Pinned. Ids are random and never reused, so a stale
  id never lands in a new folder.

## Consequences

- Rename and delete touch only the preference, so they are instant and never
  unpin a thread.
- A thread can carry a stale folder id after a delete. It is harmless: the
  client treats it as Pinned, and the next move or unpin overwrites it.
- The split into sections lives in the client. Another client must apply the
  same rule: a family follows its root, and an unknown id falls back to Pinned.
- Archive stays flat. An archived thread loses its folder.

## Alternatives considered

- **A `thread_folders` table with a foreign key from threads.** Lost: it adds a
  store and an API for a small user setting. Deleting a folder would then have
  to rewrite every member row, or cascade into an unpin.
- **A payload on `ThreadSaved`.** Lost: it changes the shape of a stored event,
  and a move between folders would need a second pin event anyway.
- **Folder sections from the engine (`DisplaySection::Folder(id)`).** Lost: the
  section key would depend on user data, and every engine section consumer
  would need to know the folder list.
- **Folders as nested groups inside Pinned.** Lost: the requester wanted
  top-level sections, and nesting adds a second collapse level to the drawer.
