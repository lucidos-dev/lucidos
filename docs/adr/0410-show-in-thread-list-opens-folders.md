# 0410: Show in Folders replaces Show in thread list, and always opens the Folders grouping

- **Status**: Accepted
- **Date**: 2026-10-10
- **Amends**: [0394](0394-drawer-ongoing-grouping-fixed-order.md) (its Show in thread list clause)

## Context

ADR 0394 kept the thread menu's Show in thread list inside Ongoing: it opened
the group that listed the thread, and switched to Folders only for a thread in
no group. In use, the user asked for the opposite. They pick the item to see
where a thread lives, and that is a Folders question.

## Decision

- The item is called **Show in Folders**, because Folders is the name of the
  view it opens.
- It always switches the drawer to the Folders grouping and reveals the row
  there. It never selects an Ongoing group, and it leaves the stored Ongoing
  selection as it was.

## Rationale

Every thread has exactly one place in Folders: its section and its family.
An Ongoing group lists a thread only while it is going on, and a thread can
sit in two groups at once. So Folders is the one answer that holds for every
thread, and the action now does the same thing every time. The label then
names that place, where "thread list" named no view the user can find.

## Consequences

- `revealOngoingGroup` is gone. The reveal has one path,
  `revealThreadInFolders`.
- A user who was working in Ongoing leaves it on Show in Folders. The grouping
  button brings them back, to the group they had selected.

## Alternatives considered

- **Keep 0394's rule.** Rejected: the user found the action did not go where
  they expected.
- **Stay in the grouping on screen, whatever it is.** Rejected: the same
  objection, for the same reason.
- **Keep the label "Show in thread list".** Rejected: both groupings are the
  thread list, so the label did not say which one it opens.
