# 0756: The gateway subscribes to a filtered engine stream for the picker row, and the two-second poll is deleted

- **Status**: Accepted
- **Date**: 2026-11-21
- **Amended by**: [0760](0760-agent-role-writes-no-table.md) (the agent role
  no longer writes `notification_reads` in a user workspace)

## Context

The gateway read two engine routes for every running engine on each
two-second supervise pass: the unread count and the backup line of a picker
row. One workspace logged 183,000 such requests in 50 hours, about 90% of its
diagnostic log. Two more callers reached the same count route: the row listing
for a paired device, on every listing, and the dock total.

Both facts already have events. The plan is
`docs/plans/2026-11-17-events-replace-the-two-polls.md`, Slice B.

## Decision

- **The gateway subscribes to the engine's existing stream, filtered.**
  `GET /api/v1/events?aggregates=notification,ops,preference,user` is the *row
  stream*. One *row subscription* per stack follows it over the engine socket.
- **A frame is a poke, never the value.** The unread count depends on the
  reader, so no event can carry it. On a poke the gateway reads the two
  existing routes again. They stay, as the snapshot.
- **The row stream sends eight event types and no payload.** A frame is the
  event's type and aggregate. Five move an unread count: `NotificationCreated`,
  `NotificationRead`, `NotificationsAllRead`, `ThreadsDeleted` and
  `UsersChanged`. Three move the backup line: `BackupCompleted`, `BackupFailed`
  and `PreferencesChanged`. The engine holds the list. Sharing an aggregate
  with one of them passes nothing.
- **A deleted thread and a users change are unread events.** The delete
  removes the thread's notifications. A users change can move a device to
  another reader, so its held count may belong to the earlier one.
- **A stream silent for three keepalive intervals is dropped.** The engine
  sends a keepalive in every quiet interval, so that much silence means a
  stall. The row blanks, and the next healthy probe subscribes again.
  `row_stream::SILENCE_LIMIT` holds the limit.
- **A row stream is not a page.** It does not register in `sse_connections`.
- **The engine advertises the stream as `event_filter: true` in its health
  body.** The gateway subscribes only to an engine that does.
- **An engine without it gets a quiet row**: no badge, no backup line, no
  device count. The poll is deleted with no fallback.
- **The gateway respawns such an engine once it is idle.** It does so once
  per engine, only for an engine it keeps, and only when the engine said it
  runs no turn. This is the idle check the stamp re-key already uses. A busy
  engine keeps its quiet row until its turns end. The gateway logs the engine
  once when it first sees it.
- **The dock total asks such an engine directly.** It has no held count, and
  a blank summed as zero would undercount in silence. Its row stays quiet.
- **The health probe stays**, every two seconds. It decides when a
  subscription opens and when it ends.
- **A hello, a lagged frame and the end of the stream each distrust
  everything held.** The first two read the snapshot again. The last blanks
  the row until the next subscription's hello.
- **Each device's own count is held between two frames that could move it.**
  A listing asks the engine only when the gateway holds none.
- **The unread-total route never uses a held count.** A push badge is sent
  once, and its request can arrive before the frame for the same event. So
  that route asks each engine every time, with or without a live stream.
- **Staleness needs no event.** The backup route names `stale_at`, and the
  gateway compares it with its own clock on each listing.

`lucidos-gateway-stamp::row_stream` holds the names both sides use. The
gateway cannot depend on `lucidos-core`, so a test beside the engine's handler
pins them to the engine's own.

## Rationale

- **Events are the authority.** Each write that moves either fact already
  announces. A second "count changed" event would be a wrapper.
- **No payload, because the stream is new surface.** A notification carries
  text for one audience and a preference carries its value. The gateway needs
  neither, and an event the reader re-reads anyway loses nothing by arriving
  empty. The allow-list keeps a later event from joining by sharing an
  aggregate.
- **Not a page, because a page can pong.** The push fan-out waits for a
  presence pong while any page is connected. A permanent gateway connection
  would delay every push.
- **The health flag, because an older engine ignores the query.** It would
  send the whole stream and count the gateway as a page.
- **A blank row is honest and a stale one is not.** Nothing would correct a
  count held with no live stream.

## Consequences

- An idle engine serves no periodic request but the health probe.
- A new gateway beside an engine built before this change shows no badge and
  no backup line for that workspace, until that engine restarts. The window
  ends when the engine goes idle, because the gateway then respawns it.
- The window stays open for an engine somebody else keeps, and for one the
  respawn brought back on the same older build. The gateway replaces
  neither. The dock total still counts both.
- The window opens only where the gateway outlives a version change: a dev
  reload, which re-execs the gateway and keeps every engine. A packaged
  update and the installer both stop every engine first.
- An older gateway beside a newer engine still polls the two routes, which are
  unchanged, as is the unfiltered stream.
- A failed snapshot read retries every two seconds while the stream is live.
  That is the old poll, bounded to the time the engine cannot answer.
- The dock total reads gateway memory. A read that races the event's own
  snapshot read sees the earlier count, until the dock's next five-second tick.
- A write that moves the unread count without one of the listed events leaves
  the row at its earlier value until the next poke. A new such write adds its
  event to `row_stream::UNREAD_EVENT_TYPES`.
- A direct SQL write as the agent role is such a write: `notification_reads`
  is agent-writable and a direct write emits nothing. The page's own count
  has the same gap. The two-second poll used to correct the picker row.

## Alternatives considered

- **Keep the poll for an engine without the flag.** Rejected: it costs a
  registered temporary measure for a window that ends at that engine's next
  restart. The row is quiet in that window, never wrong. The dock total is
  the exception, because a sum cannot show a quiet part.
- **Show the dock total as unknown while an engine lacks the flag.**
  Rejected: one busy older engine would blank the total of every workspace.
- **Send the selected events whole.** Rejected: it puts audience-targeted
  text and preference values on a stream whose reader needs none of it.
- **Filter by aggregate alone.** Rejected: `ops` and `notification` also
  carry the backup-key reveal and the native push request.
- **A new "picker row changed" event.** Rejected as a wrapper around events
  that already exist.
- **Give the gateway a database handle.** Rejected by ADR 0014.
- **A second engine route for the row stream.** Rejected: the existing stream
  already has the hello, the lagged frame and the keep-alive.
