import { hasHoverPointer } from '../../utils/platform';
import { isElementOnScreen, isElementVisible } from './scrollState';
import { getVisiblePromptInput } from './promptFocus';

/** Keyboard navigation for **choice cards**: a live user question card or a live
 *  permission card (see `docs/glossary.md`). Both park the thread on
 *  `waiting_for_user_answer` and offer a fixed set of on-card choices, so both
 *  want the same contract:
 *
 *   - one choice carries real DOM focus the moment the card appears, so Enter
 *     answers immediately (the button's own native activation, no key handling),
 *   - arrow keys step between the choices,
 *   - the focus ring is VISIBLE wherever the card can be seeded. Ordinary
 *     keyboard focus rings via an ungated `:focus-visible`; the programmatic
 *     seed, which that heuristic does not reliably match, rings via a plain
 *     `:focus` gated on `@media (hover: hover)`, the CSS twin of the
 *     `hasHoverPointer()` gate below (see `styles/chat/response.css`). A seeded
 *     focus the user can't see is an Enter whose effect is hidden, and that
 *     visibility is what makes seeding "Allow once" on a permission card
 *     acceptable rather than a foot-gun.
 *
 *  A card opts in by putting `data-role="card-choices"` on the element wrapping
 *  its choices, and only while it is LIVE. An answered or terminated card must
 *  never be a focus target, or arrowing through history would land on it. A card
 *  may mark one button `data-default-choice` to be seeded instead of the first. */

/** `data-role` on the element wrapping a LIVE choice card's buttons. */
export const CHOICE_CARD_ROLE = 'card-choices';

const CHOICE_CARD_SELECTOR = `[data-role="${CHOICE_CARD_ROLE}"]`;
/** Enabled choices within a marked container, in DOM order. Disabled buttons are
 *  excluded so a card mid-resolution (every button disabled) navigates nowhere. */
const CHOICE_SELECTOR = 'button:not([disabled])';

/** Prev/next direction for an arrow key. BOTH axes map to the same walk: the
 *  question card is a vertical list while the permission card mixes a horizontal
 *  primary row with stacked secondary rows, so there is no single axis that
 *  reads correctly on both. */
function arrowDelta(key: string): 1 | -1 | null {
  if (key === 'ArrowDown' || key === 'ArrowRight') return 1;
  if (key === 'ArrowUp' || key === 'ArrowLeft') return -1;
  return null;
}

/** Clamped step, matching `ThreadDrawer.moveHighlight` and `Dropdown`: no wrap,
 *  and a `current` of -1 (focus not on any choice) seeds to the first going
 *  forward, the last going backward. */
function stepChoiceIndex(current: number, delta: 1 | -1, count: number): number {
  if (current < 0) return delta > 0 ? 0 : count - 1;
  return Math.max(0, Math.min(count - 1, current + delta));
}

/** Which choice index an arrow keydown moves to, or null when the card must not
 *  handle the key at all. A chord carrying a primary modifier or Alt is a global
 *  shortcut (turn nav, history, maximize pane group) and must bubble untouched,
 *  the same guard `ThreadDrawer.handleKeyDown` uses. Pure, for unit testing. */
export function nextChoiceIndex(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
  current: number,
  count: number,
): number | null {
  if (e.metaKey || e.ctrlKey || e.altKey) return null;
  const delta = arrowDelta(e.key);
  if (delta === null || count <= 0) return null;
  return stepChoiceIndex(current, delta, count);
}

/** Whether the arriving card may take DOM focus. Every clause is a refusal to
 *  steal focus from something the user is doing, mirroring `focusIfNeeded` and
 *  `shouldReconcilePaneFocus`: a touch-only device has no keyboard to serve, a
 *  non-empty prompt means they are composing a free-text answer, a non-idle
 *  active element means they are using some other control. Whether the card is
 *  actually on screen is asked separately and precisely, by `isElementOnScreen`
 *  in `seedChoiceCardFocus`. Pure, for unit testing.
 *
 *  `hoverPointer` is `hasHoverPointer()`, NOT `isMobile()`: the question is
 *  whether a keyboard exists to press the seeded choice with, and `isMobile()`
 *  only measures viewport width. An iPad in landscape is wider than the mobile
 *  breakpoint and still has no keyboard, so a width gate would programmatically
 *  focus a button there and leave a stray ring, which is precisely what
 *  `hasHoverPointer` was extracted to prevent. It is also the JS mirror of the
 *  `@media (hover: hover)` gate on the ring itself, so the two cannot drift. */
export function shouldSeedChoiceFocus(opts: {
  hoverPointer: boolean;
  promptHasText: boolean;
  activeIsIdle: boolean;
}): boolean {
  return opts.hoverPointer && !opts.promptHasText && opts.activeIsIdle;
}

function choiceButtons(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(CHOICE_SELECTOR));
}

/** The choice a card seeds focus onto: its declared default, else the first.
 *  The marker is how the two card kinds differ without the nav code knowing
 *  either of them. A question card declares none (so the first option wins); a
 *  permission card marks "Allow once". */
function defaultChoice(root: HTMLElement): HTMLElement | null {
  const marked = root.querySelector<HTMLElement>('button[data-default-choice]:not([disabled])');
  return marked ?? choiceButtons(root)[0] ?? null;
}

/** True when the user is composing, so the seed must leave focus alone. Reads
 *  the textarea rather than the draft store to keep this module free of a store
 *  import (and correct for a keystroke not yet flushed to the draft). */
function promptHasText(): boolean {
  const el = getVisiblePromptInput() as HTMLTextAreaElement | null;
  return (el?.value ?? '').trim().length > 0;
}

/** True when DOM focus sits on nothing the user is actively using: no active
 *  element, the document/body, the transcript scroll region (where
 *  `reconcilePaneFocus` parks it), or the prompt textarea. Anything else (an app
 *  iframe, a settings field, a drawer row) is theirs and must not be taken. */
function activeElementIsIdle(): boolean {
  const active = document.activeElement as HTMLElement | null;
  if (!active || active === document.body || active === document.documentElement) return true;
  if (active.dataset?.role === 'prompt-input') return true;
  return active.classList?.contains('thread-content') === true;
}

/** Move focus one choice along the arrow's direction and consume the keystroke.
 *  Wire as the `onKeyDown` of the `data-role="card-choices"` element, so it only
 *  ever sees keydowns that originated inside the card. */
export function handleChoiceCardKeyDown(e: KeyboardEvent, root: HTMLElement | null): void {
  if (!root) return;
  const items = choiceButtons(root);
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next = nextChoiceIndex(e, current, items.length);
  if (next === null) return;
  // Consume even at the clamp, so an arrow held at the end of the list doesn't
  // suddenly start scrolling the transcript out from under the card.
  e.preventDefault();
  if (next === current) return;
  // Focus without the browser's own reveal, then reveal deliberately, exactly as
  // ThreadDrawer / Dropdown / SearchEverywhere do. Both halves are needed: the
  // `preventDefault` above killed the transcript's native arrow scroll, so
  // without an explicit reveal a card taller than the visible transcript would
  // step focus onto an option above or below the fold, leaving the user with an
  // invisible ring and an Enter whose target they cannot see. `block: 'nearest'`
  // keeps an already-visible option perfectly still.
  items[next].focus({ preventScroll: true });
  items[next].scrollIntoView({ block: 'nearest' });
}

/** Card ids whose arrival moment has already passed. See `claimSeedForCard`. */
const seededCards = new Set<string>();

/** Claim the one-and-only seed for a card, returning true the FIRST time it is
 *  asked about a given id and false forever after.
 *
 *  The seed belongs to a card's ARRIVAL, and a card can become live more than
 *  once. Answering is optimistic, so a failed send rolls the answer back and
 *  the card returns to live: a permission card's buttons re-enable, and a
 *  question card's option list remounts. Without this latch that rollback
 *  re-seeds the DEFAULT choice, which on a permission card means focus jumping
 *  to "Allow once" moments after the user pressed Deny and the send failed.
 *  The always-visible ring is no defence there, because the user has just
 *  expressed the opposite intent and has no reason to re-read the card.
 *
 *  It latches on the first ASK, not on the first successful focus, because the
 *  guards below can legitimately decline an arrival (the user was composing)
 *  and a later rollback is still not a new arrival. Ids accumulate for the life
 *  of the page, which is a few dozen bytes per card answered. Pure enough to
 *  unit test; the DOM work lives in `seedChoiceCardFocus`. */
export function claimSeedForCard(cardId: string): boolean {
  if (seededCards.has(cardId)) return false;
  seededCards.add(cardId);
  return true;
}

/** Seed focus onto a freshly-arrived live card's default choice. Call from the
 *  card's mount effect, passing the card's stable id (`tool_use_id` for a
 *  question, `request_id` for a permission request).
 *
 *  Two guards, doing two different jobs. `claimSeedForCard` bounds this to the
 *  card's arrival, and `isElementOnScreen` proves the choice is actually
 *  visible, so we never arm an Enter the reader cannot see.
 *
 *  There was a third, a position SIGNAL (`scrolledUp`, the 80px stickiness
 *  window) standing for "the reader has chosen to read history, do not hijack
 *  their arrow keys". It went with the bottom-pin that maintained it. Carrying
 *  it over to `awayFromBottom` would have been worse than dropping it: nothing
 *  scrolls to a card now, so the card's own arrival puts the reader off the
 *  bottom, the resize handler sets `awayFromBottom` before this effect runs, and
 *  `claimSeedForCard` has already latched the id. Every card in a scrollable
 *  thread would decline its one chance at focus, permanently. `isElementOnScreen`
 *  answers the real question, and answers it about the choice rather than about
 *  the transcript.
 *
 *  `preventScroll` then keeps the (already-in-view) focus move from nudging the
 *  scroll manager, matching `focusIfNeeded`. */
export function seedChoiceCardFocus(root: HTMLElement | null, cardId: string): void {
  if (!root || !claimSeedForCard(cardId)) return;
  if (!shouldSeedChoiceFocus({
    hoverPointer: hasHoverPointer(),
    promptHasText: promptHasText(),
    activeIsIdle: activeElementIsIdle(),
  })) return;
  const choice = defaultChoice(root);
  if (choice && isElementOnScreen(choice)) choice.focus({ preventScroll: true });
}

/** Where DOM focus belongs when a thread becomes the focused one: the default
 *  choice of a live card if the thread is parked on one, else the prompt.
 *
 *  This is the SINGLE decision point for that question. The card's own mount
 *  seed and `PromptInput`'s thread-switch `focusIfNeeded` both fire on a switch,
 *  and which one lands last depends on mount order, so the prompt asks here
 *  instead of racing. Takes the LAST visible marked card (the most recent one,
 *  should a superseded card still be live) and falls back to the prompt when the
 *  card carries no options at all (a free-text-only question), or when it is
 *  off screen: a reopened thread restores its remembered scroll position
 *  (`useScrollMemory`), which can leave a live card below the fold, and focusing
 *  it there would arm an invisible Enter over a question answer or a permission
 *  grant. Falling back to the prompt is the fail-safe: the user scrolls to the
 *  card and answers it the way they always could.
 *
 *  It deliberately does NOT apply `activeElementIsIdle`, unlike the arrival
 *  seed. This runs on a thread SWITCH, where the active element is by
 *  construction whatever caused the switch (the drawer row just clicked, a
 *  search result, a notification deep link), so an idle check would be false
 *  every time and the card would never be reached. The switch already moves
 *  focus regardless: the pre-existing behavior was to focus the prompt
 *  unconditionally, and this only changes WHERE focus lands. */
export function threadEntryFocusTarget(prompt: HTMLElement | null): HTMLElement | null {
  if (!hasHoverPointer() || promptHasText()) return prompt;
  const roots = Array.from(document.querySelectorAll<HTMLElement>(CHOICE_CARD_SELECTOR))
    .filter(isElementVisible);
  const last = roots[roots.length - 1];
  const choice = last ? defaultChoice(last) : null;
  return choice && isElementOnScreen(choice) ? choice : prompt;
}
