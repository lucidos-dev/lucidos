# 0141: A park hides its windows, and a reopen restores the arrangement

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

Cmd-Q in the packaged macOS client is bound to **Close to Menu Bar**, not to
Quit. `close_all_to_tray` hid `main` and **closed** every secondary window. Each
close fired `Destroyed`, `forget_closed_window` re-captured the live set, and the
*window session* (ADR 0123) shrank to `main`'s workspace alone.

So a user with three workspace windows got one back, and got one back twice
over. The tray reopen fronted `main` and nothing else. A later relaunch also gave
one window, because the record had already shrunk while the client sat in the
menu bar.

ADR 0123 recorded this as an accepted limitation and named the fix: restore the
parked windows on the tray reopen, as a separate change. This is it.

The rule that put the close there predates the window session. The plan
`docs/plans/2026-07-01-macos-client-menu-bar-only-on-window-close.md` wrote it as
"`main` is hidden (orderOut), never destroyed, on the close-to-tray paths; only
secondary New-Window windows are destroyed". That was right at the time. Nothing
then remembered a window per workspace, so a secondary window had no identity
worth keeping.

## Decision

A park **hides every app window and destroys none**. `main` loses its special
case rather than gaining company.

A reopen puts back the whole arrangement. It shows every parked window, and
builds any workspace the record names that this process no longer holds. It is
what the tray's "Open Lucidos" and the Dock click both mean.

## Rationale

**A hidden window is already part of the arrangement.** ADR 0123 says so, and the
window session's write gates are built on it: neither half asks whether a window
is visible, and a live visibility test was tried and reverted. `main` has always
been hidden rather than closed. Extending that to the secondaries makes the code
smaller, not larger.

**The record needs no new state, because nothing takes anything from it.** No
`Destroyed` fires during a park, so no re-capture runs and the record still names
every parked workspace. A relaunch after a park therefore restores too, for free
and through the path that already existed.

**This is not the counter ADR 0123 warns about.** `PARKED_WINDOWS` stood the
re-capture down so the record would keep naming windows the client had already
destroyed. The record then claimed three windows while the reopen produced one,
and each round of patching that disagreement opened another hole. Here the
disagreement cannot arise: the windows are still there, so the record describing
them is simply true.

**The reopen reads the live windows first and the record second.** A window that
exists is shown, never rebuilt, which is what preserves its page state and makes
a park cheap to undo. The record is consulted only for a workspace no live window
is on. That case has one real cause. The login agent's launch comes up
menu-bar-only and restores nothing (ADR 0072). So the first reopen after a reboot
is where the windows are finally owed.

**An adrift `main` is navigated, not left behind.** A `main` on the picker or the
gateway root takes the first owed workspace itself. Building a window for it
instead leaves a stray picker window behind the restored ones, which is the
defect v0.30.4 fixed for the picker rows.

**A notification tap is untouched.** It names one workspace, and
`route_native_tap` still fronts the window on it. ADR 0123 already gives a tap
precedence over the session. Raising every parked window over the user's work is
not what tapping one banner asked for.

## Consequences

- **Cmd-Q is now reversible.** Park three windows and any reopen gives three
  back, each on its workspace, at its frame, with its scroll and route intact.
- **A relaunch after a park restores too**, because the record no longer shrinks
  while the client is parked. That closes the second half of the same report.
- **The first reopen after a reboot restores the arrangement.** The login start
  itself is unchanged: still menu-bar-only, still no window.
- **A parked client holds every webview, not one.** Three parked workspace
  windows keep three webviews and three gateway connections alive overnight,
  where the close freed two of them. That is the price of the instant reopen with
  page state intact, and it is the trade already made for `main`.
- **Two windows on one workspace both survive a park.** The reopen replays the
  live windows rather than the record, and only the record collapses them.
- **A notification tap can leave part of the desk parked.** The tapped window
  comes forward and the rest stay hidden. The tray item brings them back.
- **A reopen while the service is still starting gives the boot splash, not the
  arrangement.** `launch` owns `main`'s first navigation and is about to restore
  the same workspaces, so the reopen defers rather than racing it. It is the
  likeliest reopen after a reboot, since the login agent sits in the menu bar
  while the service starts. The cost is one more click once the client is up.
  Aiming the boot navigation, the way a tap does, would mean teaching the login
  start to restore after all. That is a separate change.
- **A window shown by a reopen is not told it is active.** It is on screen but
  unfocused, so `native-window-active` stays false. The engine keeps sending it
  OS banners rather than a toast nobody is looking at, and only the fronted
  window is told otherwise.
- **`close_all_to_tray` no longer reaches the `Destroyed` arm at all.** The
  hazard that arm carried during a park is gone with it: a queued
  `CloseRequested` finding an earlier window half torn down, which needed its own
  fix while the park still closed windows.
- **One `window_order_key` serves the capture and the restore.** The order a
  record is written in cannot drift from the order it is read back.

## Alternatives considered

**Re-add the park counter.** Hold the record across a park and let a relaunch
restore from it. Rejected, and ADR 0123 already rejected it. It leaves the record
saying something the client is not showing, so every later decision built on the
record has to special-case the park. It also does nothing for the tray reopen,
which is the case the user actually hits.

**Close the windows, but remember what was closed.** Keep the memory saving, and
rebuild on the reopen from a parked list. Rejected on two counts. That list is
new state, and it has to stay true against every window the user closes, opens or
navigates while parked. This is the counter's failure mode wearing a different
hat. Every reopened window is also a cold load, so the secondaries would never
get the fast reopen the July plan promised for `main`.

**Restore the whole arrangement on a notification tap too.** Consistent, in the
sense that every "bring the client back" path would behave alike. Rejected: a tap
requests one workspace, and answering with five windows over the user's work is
not consistency, it is an ambush.

**Build the recorded windows hidden at login, ready for the first reopen.** It
would make the reboot reopen instant. Rejected: it puts N webview loads and N
gateway connections on every login, for a client the user may never open that
day. ADR 0072's whole point is that a login start costs nothing.
