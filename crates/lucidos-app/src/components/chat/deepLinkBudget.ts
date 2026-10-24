/** How long a deep link waits for its target to appear before giving up. It is
 *  the budget for waiting out ONE lazily loading thread. Exported so a caller
 *  with its own pre-navigation wait on the same thread spends the same budget
 *  rather than a second literal that drifts. `showEventWhereItLives` is that
 *  caller, resolving the anchor before it focuses.
 *
 *  Apart from `scrollState.ts` so a store action can read it without pulling
 *  the scroll engine into the entry chunk (`scrollStateHandle.ts`). */
export const EVENT_RESOLVE_DEADLINE_MS = 4000;
