# 0574: The stranded follow-ups check retries a dropped connection, then fails the turn rather than start without it

- **Status**: Accepted
- **Date**: 2026-10-25

## Context

Before a coding-agent session starts, the engine asks the event store which
follow-ups the previous session took and never read (`stranded_follow_ups`).
The new session then sends them again.

One such turn failed with "Could not check for messages the previous session
never read: error communicating with database: expected to read 5 bytes, got 0
bytes at EOF". The Postgres log showed the cause. The host's out-of-memory
killer sent SIGKILL to one Postgres backend, which was running an unrelated
query. Postgres then ended every other backend and ran crash recovery, which
took about two seconds. The check's connection was one of those it ended.

The pool was not at fault. sqlx tests a connection before it hands it out
(`test_before_acquire` is on by default), and again on release. The engine
pool also sets `idle_timeout` and `max_lifetime`. No pool setting can save a
query whose server process dies while it runs.

## Decision

The check runs through `core::db_retry::retry_transient_read`: up to three
attempts, 200 ms then 400 ms apart, retrying only a transient error. If it
still fails, the turn fails as before, with the same message.

## Rationale

**Retry**: a crash restart ends every connection at once, so the first attempt
fails and a second one, on a fresh connection, succeeds. The pause can be short
because sqlx's acquire already waits while the server is still recovering: it
retries a connect refused with SQLSTATE 57P03 until its 60 second timeout. For
the same reason a timed-out acquire is not retried: it has already waited that
full minute, and two more attempts would only make the turn hang three times as
long before it fails.

**Fail the turn, never degrade**: the session's `SessionStarted` closes the
window the check reads. If a session starts without the answer, every stranded
follow-up looks settled from then on. The user's message is lost, and nothing
says so. A failed turn is loud and costs one resend, and the next turn runs the
check again. The session start needs the database next anyway, so after three
failed attempts it would most likely fail regardless.

## Consequences

- A Postgres crash restart during a session start costs a short pause, not a
  failed turn.
- A deterministic error, such as a syntax error, still fails at once and names
  itself.
- `is_transient_sqlstate` is now the one list of retryable SQLSTATE classes.
  The boot failure report and the retry helper both use it.
- The retry is for reads only. A write may have landed before its connection
  dropped, so running it again could apply it twice.

## Alternatives considered

- **Change the pool settings.** Rejected: the settings were already right, and
  none of them covers a backend that dies mid-query.
- **Start the session and log a warning.** Rejected: it trades a loud,
  recoverable failure for silent loss of the user's message.
- **Retry every query at the pool level.** Rejected: sqlx has no such hook, and
  a blanket retry would also repeat writes. Other reads can adopt the helper
  where a failure costs the user a turn.
