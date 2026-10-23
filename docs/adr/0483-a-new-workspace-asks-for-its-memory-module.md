# 0483: A new workspace asks for its memory module in the memory choice; unset runs Classic

- **Status**: Accepted
- **Date**: 2026-10-14

Amends [ADR 0362](0362-tree-memory-module-and-the-home-thread.md) § Amendment,
2026-10-12: Tree is the default. Plan:
`docs/plans/2026-10-14-v0-49-0-release-blockers.md`, Part 2.

## Context

ADR 0362's 2026-10-12 amendment made Tree the default for a new workspace.
Tree costs money: a compactor model writes summaries after each turn. A new
user started paying without being asked, and without seeing a price. The
upgrade path already refused to do that to an existing workspace.

## Decision

`memory_module` has no default. While it is unset the engine runs Classic,
which is almost free, and starts no compactor. The app asks in a modal, the
*memory choice*, which shows what Tree costs a month for three
ways of using a workspace. Every answer writes the preference. Closing the
modal is a Classic answer, so it asks once.

## Rationale

- **Nothing is spent on Tree before the user answers.** Unset runs Classic,
  which is almost free: on long threads it makes a small summary call now
  and then.
- **The price is shown where the choice is made.** A monthly figure per usage
  profile is what a user can weigh. The estimate prices one turn from the
  measured seed of compactor calls per turn, so a workspace with no history
  still gets a real figure.
- **The stored value is the only state.** No "asked" flag. A peer device's
  answer arrives as a preference change and closes the modal there too.
- **The agent does not choose.** An agent writing `memory_module` skips the
  cost the modal shows, so the knowhow points the user at the memory choice
  or Settings.

## Consequences

- A headless or CLI workspace runs Classic until a client opens it.
- An upgraded workspace keeps the `classic` the 2026-10-12 migration stored,
  so it is never asked. The release notice asks it to try Tree from Settings.
- The release notice for Tree now holds only for a stored Classic. A workspace
  that never chose meets the memory choice instead.
- The e2e reset stores `classic`, so the modal stays out of unrelated specs.

## Alternatives considered

- **Keep Tree the default.** Rejected: it spends the user's money unasked.
- **Ask in the Home welcome.** Rejected: a phone opens Home on the board face,
  so the welcome sits out of sight. The welcome also retires on the first
  message, so typing first would skip the question unseen.
- **Ask in the setup interview.** Rejected: the interview runs only on
  request, and an agent writing the preference skips the cost.
- **Store an "asked" flag beside the preference.** Rejected: two states that
  can disagree. Closing writes `classic`, which says the same thing.
