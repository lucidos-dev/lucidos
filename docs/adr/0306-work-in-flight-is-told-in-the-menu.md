# 0306: Work in flight is told in the Lucidos menu, never in a progress toast; the Expose run keeps its toast

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

Long-running work had three homes. Applies raised a spinner toast per thread
and a batch toast for an Apply All. The embedding download opened a status toast
by itself. Builds showed a popover hung from the brand badge. The *activity
group* then gave every job a row in the Lucidos menu. So the same work showed in
a toast and in the menu at once.

The user expected the building and applying activity to be contained in the
menu. Asked directly, they chose a line per job that unfolds its detail in
place, like the Workspaces row, and no progress toast at all. Failures and
results still pop up. The plan is
`docs/plans/2026-09-27-work-in-flight-unfolds-in-the-menu.md`.

## Decision

The activity group is the one place work in flight is told. Each row unfolds
its detail under itself: an Apply All's position, thread and Cancel; a build's
changelog; the download's progress. No apply, build or download raises a toast
while it runs. Results and failures still toast. The Expose run is the one
exception, and keeps narrating in its own toast.

## Rationale

- **One place to look.** Two surfaces narrating the same work is noise, and the
  toast covered the transcript for work the user did not ask to watch.
- **Inline beats a second layer.** A row that unfolds keeps the menu open. A
  popover opened from a menu swapped one layer for another (ADR 0290: detail
  drills in).
- **A result is news, progress is not.** "Applied", a failure and the batch
  summary still reach the user unprompted, and a refetch reports an apply that
  landed while the page was away.
- **Expose blocks on the user.** One of its steps waits, for minutes, until the
  user opens a tailnet approval link. Settings shows no link, so a narration
  hidden in the menu would stall the run with no sign why.

## Consequences

- The background-activity popover and the batch toast are gone, with their
  close state. Cancel for an Apply All lives in its menu detail.
- The apply estimates of ADR 0305 show in the unfolded apply line: the time a
  phase has run, how long it usually takes, and the batch's time left.
- The shared background-activity toast narrates only the Expose run now. A
  watched download reports its outcome in a toast of its own, once.
- A user who never opens the menu sees the spinning badge, then the result. That
  is the intended weight for work nobody asked to watch.

## Alternatives considered

- **Keep the toasts and add the menu rows.** Rejected by the user: the same work
  showed twice.
- **A menu row that opens the existing toast.** Rejected: a toast reopened from
  a menu is still a second surface, and single-thread applies had no way back.
- **A popover per job, hung from the mark.** The shipped shape before this. It
  closed the menu to open a second layer.
- **Drop the Expose toast too.** Rejected: its approval link would reach the user
  only if they thought to open the menu.
