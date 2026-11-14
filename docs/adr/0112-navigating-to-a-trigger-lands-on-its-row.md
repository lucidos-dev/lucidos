# 0112: Navigating to a trigger lands on its row in the panel, not in its edit form

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

`navigateToTrigger` is the one route to a trigger. Six things reach it. A
`trigger:<id>` link in a chat reply, a notification tap, and the notification
detail's **Open trigger** button. Then a Search Everywhere hit, the
message-route panel, and `navigate_ui`'s `trigger` target.

It used to set `panelOverlay` to the trigger **form**, so all six landed the
user in `TriggerDetails`: cron expressions, the intent textarea, the model
picker, the side-effect grant. That is the surface for changing a trigger's
configuration.

The prompt to look at it was a bug report. A trigger's Python script failed, the
agent fixed it, and told the user to "hit **Run once** on the trigger in
`[Triggers](triggers)`". The panel link was the only trigger-shaped link the
agent had, and adding `trigger:<id>` raised the question the report had already
answered by implication: **Run once is not in the form.** It is on the row,
along with the pause toggle, the last-run OK or failed chip and the schedule.

There is a second reason the question is worth recording rather than just
fixing. `docs/plans/2026-06-30-unified-navigation-focus-marker.md` put triggers
explicitly **out** of the navigation focus marker's scope, and gave a reason:
`navigateToTrigger` is a whole-pane open with "no inner row to mark", the same
category as an app or a file preview. That reasoning was correct about the code
as it stood, and it stops being correct the moment the landing changes.

## Decision

**Navigating to a trigger opens the Triggers panel and lands on that trigger's
row**, scrolled into view and marked with the shared *navigation focus marker*.
No overlay form is opened, so a form left open on a different trigger closes.

All six entry points keep going through `navigateToTrigger`, so they all land
the same way. `openEditTrigger` is untouched: clicking the row still opens the
form, which is therefore one tap from the landing.

**Triggers are in the navigation focus marker's scope**, alongside a chat event,
a settings item and a plugin row.

## Rationale

**A pointer lands where the thing's affordances are.** "Here is your trigger" is
almost always followed by an action: run it now, pause it, see whether the last
run failed. Every one of those is on the row and none is in the form. Landing in
the form makes the user close it to reach what they were pointed at.

**A form is a bad place to arrive by accident.** It is a mutable surface with a
Save button. Arriving there from a tap on a notification, and leaving by any
route other than Cancel, invites an edit nobody asked for.

**The row carries context the form drops.** Its group, its neighbours, its
status relative to the other triggers. A user who followed a link about one
trigger often wants exactly that.

**The marker was already built for this shape.** A plugin row and a settings
item get the flash-then-stick because a row inside a list blends in. A scroll
with no mark leaves the user hunting. A trigger row has that same problem.

## Consequences

- **The trigger form is one tap further away** from every navigation entry
  point. Accepted: reaching the form is an editing intent, and editing intent
  starts from the panel.
- **`navigateToTrigger` needs the panel to render before it can finish.** The
  landing is therefore two steps: the action stamps `triggerScrollTarget`, and
  `TriggersView`'s effect consumes it once the rows mount. The same shape as
  `pluginScrollTarget`.
- **A collapsed group has to be expanded first.** `TriggersView` renders no
  members of a collapsed group, so the row's anchor does not exist to scroll
  to. The effect expands the group and lands on the following render. Missing
  that step would reproduce the reported bug by another route: a link that
  looks live and does nothing.
- **The consume-once contract is load-bearing.** A target naming no row is
  dropped rather than held, or a stale id would mark an unrelated row the next
  time the panel opens.
- **The earlier plan's exclusion line is superseded**, on triggers only. Apps
  and file previews stay out of the marker's scope: they really are whole-pane
  opens with no inner row.

## Alternatives considered

- **Keep the form, and give the chat link alone the row landing.** Rejected: two
  behaviours under one name. A chat link and the notification's Open trigger
  button would reach the same trigger by different doors, and neither surface
  says which door you get.
- **Land on the row AND open the form.** Rejected: the form covers the panel, so
  the mark and the scroll are invisible. It is the form landing with extra work.
- **Add a Run once button to the form**, keeping the form as the landing.
  Rejected: it treats one missing affordance as the problem. The pause toggle
  and the last-run status are missing too, and duplicating the row's whole
  action set into the form is how two surfaces drift.
- **A separate read-only trigger detail view** to land on, distinct from both
  the row and the edit form. Rejected as a third surface showing what the row
  already shows. Worth reopening only if the row runs out of space for
  something a link genuinely needs to point at, such as run history.
