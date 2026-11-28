# 0130: A workspace entering the release-notice sequence owes the whole backlog

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

A *release notice* is shown once per workspace, on the first open after an
upgrade. The workspace's place in the sequence is one preference, the *release
notice cursor*, holding the id of the last notice it answered.

A workspace with no cursor has to be given one at startup, and the first rule
shipped read: a workspace that has ever held a thread is stamped past every
notice whose `since` is strictly before the running release. A workspace with no
threads is stamped past everything visible.

The first authored notice never reached anybody. It says `since = "0.29.0"` and
shipped in v0.30.1, so it is strictly before the running release. Every workspace
with content was stamped past it on its first boot, silently. The only way to it
was Settings > System > What's New, which is the fallback surface.

## Decision

Only a workspace with **no content** is stamped. A workspace that has been used
is left with no cursor, so it owes every visible notice, oldest first.

Workspaces already stamped are repaired by a migration,
`20260825153250_clear_seeded_release_notice_cursor.sql`. It deletes the cursor of
a workspace that has threads and no `ReleaseNoticeResolved` event.

## Rationale

`since` is a floor, not a stamp (`release-notices.toml`). A notice names the
release it applies FROM, and may deliberately name an older one to reach everyone
already past it. The strictly-before rule read the same field as "the release
this shipped with", which cancels that out.

Under a floor, the honest question at placement time is what the workspace has
already been told. The answer is nothing. The sequence starts the day this engine
first boots, whatever release the workspace happens to be on, so there is nothing
to mark answered.

The fresh-workspace stamp survives for a different reason, which the floor does
not touch. An empty workspace has nothing to audit and no settings to migrate. A
modal over its first-run welcome would ask for work that does not exist.

The repair is a migration because nothing else reaches an affected workspace. The
placement leaves any workspace holding a known cursor alone, which is what makes
it a no-op after the first boot. So fixing the rule alone would have left the
notice invisible to exactly the installs that already took it.

## Consequences

- A workspace upgrading across several releases gets every notice it crossed, in
  order, rather than the current release's alone. That is the backlog rule the
  feature was designed around, and the stepper already walks it.
- One preference row is deleted on the boot that carries the migration. A cursor
  the user earned survives, because answering emits `ReleaseNoticeResolved` and
  the stamp is silent.
- A stored cursor this build cannot place is no longer rewritten on a used
  workspace. It reads as nothing answered, which owes the backlog again. The
  value is left alone rather than replaced with a position nobody reached.
- Authoring guidance is unchanged: write `since` as the release in hand, and an
  older one only to reach workspaces already past it.

## Alternatives considered

**Move the notice's `since` to the release in hand (0.30.1).** It repairs
nothing, because every affected workspace already holds the stamp. It also hides
the notice from anyone still on 0.29.x. And it re-arms the same trap for the next
workspace that enters the sequence at a higher release.

**Keep the strictly-before rule and accept the loss.** Anyone skipping a release
loses every notice authored in between, silently. A notice is the only surface
that reaches a user unprompted. Losing one leaves the drift it warned about
sitting in the workspace, with nothing to see.

**Repair by re-deriving the cursor at startup instead of a migration.** The
placement would then decide whether a stored cursor was earned or stamped, on
every boot forever. It would carry that branch for a state only one release can
produce. A migration states the same thing once, and is auditable in the tree.

**Delete the cursor for every workspace, without the guards.** It re-asks a user
who already ran the audit. It also clears a fresh workspace's stamp, which is the
one thing keeping a modal off the first-run welcome.
