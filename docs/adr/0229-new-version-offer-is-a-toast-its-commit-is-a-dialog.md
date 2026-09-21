# 0229: The new-version offer stays a toast; taking it opens a dialog that says what the switch brings

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

A ready engine version announced itself three ways, and none of them said what
was in it. The `New version available.` toast offered a Switch that restarted on
the spot. The Lucidos menu's Restart row wore a `New version` pill and answered a
tap with a bare `OK`. Settings, System opened a confirm listing the applied
changes, and was the only surface that showed anything at all.

The engine has measured the range the whole time. `version_status` carries
`pending_commits`, grouped into New, Fixed, Improved, Other and Housekeeping,
and the `Building new version` toast renders it while a rebuild runs. It was
read out during the build and then dropped, at the moment the switch became
available.

The toast, banner and dialog taxonomy
(`docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md`) files this offer as a
toast: transient, deferrable, ignorable. It names four flows entitled to a
dialog, and the pre-switch offer is not one of them.

## Decision

The offer stays a toast. What changes is what its button does: it opens the
existing restart confirm, which grows a second shape naming the version and
listing the commits the switch brings. The Lucidos menu's Restart row opens the
same confirm while it wears the pill, closing the menu first.

No modal is ever raised on its own. Nothing announces a version except the toast
and the badge, exactly as before.

## Rationale

A dialog is for a message the user cannot work around, and an announcement they
did not ask for is not that. So the arrival keeps the surface the taxonomy gives
it. What the taxonomy does not cover is the moment AFTER: the user has reached
for the offer, and a restart tears down every running session. That is a
deliberate, disruptive commit, and it is the ordinary place for a confirm.

The pattern already exists one control over. Settings' `Restart Engine` button
opens `Restart engine?` and commits on `Restart`. Pointing the toast at the same
function makes three surfaces one dialog, rather than three shapes of the same
question.

Reusing that confirm rather than building a modal is what keeps this small.
`showConfirm` already takes a title, a cancel label and grouped details, and
`.confirm-details` already scrolls inside the capped box. No component and no
CSS were added.

The action keeps its canonical name. *New version available / Switch to new
version* is a term in `system-knowhow/glossary.md`. `engineEventExplainers` also
quotes the button back to the user when a switch interrupts a turn. Renaming it
to *Update* would have moved three surfaces and a sentence in the transcript,
for a word.

## Consequences

- Taking the offer costs one more tap than it did. That tap buys the list, and
  it is the tap that makes an engine teardown deliberate.
- `confirmAndRestartEngine` is now the single confirm-then-restart entry point
  for every surface, so a fourth one inherits both shapes for free.
- The engine computes `pending_commits` whenever a switch is on offer, not only
  while a build runs. One more term on a gate whose whole job is to keep an idle
  workspace from forking `git log` per poll.
- The client keeps the range in `enginePendingCommits`. It could not live in
  `engineBuildDetail`, which is nulled the instant a build ends.
- Declining the confirm is not a dismissal. The toast stays up and the build is
  not remembered as deferred, so the offer is exactly where it was.
- A packaged build reports no commits, so its confirm falls back to the applied
  changes grouped by thread. That is the body it already had.

## Alternatives considered

**Raise the modal by itself when a version lands.** Rejected by the user when
asked, and by the taxonomy on its own terms: an offer with a `Later` on it is
ignorable by definition, and a modal is the one surface that cannot be ignored.
A workspace applying changes all day would be interrupted by each of them.

**A bespoke new-version modal.** It would have needed a component, a CSS block
and its own dismiss wiring, to arrive at the shape `ConfirmDialog` already
renders. It would also have put two dialogs in front of one decision, since
Settings' confirm would still exist.

**List the commits in the toast itself.** The toast already does this for the
BUILD, and that is the ceiling of what the surface can carry: it is capped at
14rem and scrolls nothing. A range spanning several Applies does not fit, and
the offer has to stay ignorable, which argues against making it taller.

**Only the menu row opens the confirm.** The toast would still restart on one
tap. One decision would then carry two weights, depending on which surface the
user happened to be looking at.

**Rename the action to Update.** Covered under Rationale. Available later as a
deliberate vocabulary change, with the glossary and the explainer moving in the
same commit.
