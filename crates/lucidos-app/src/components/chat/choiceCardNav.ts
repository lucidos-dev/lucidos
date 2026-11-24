import { hasHoverPointer } from '../../utils/platform';
import { isElementOnScreen, markRevealScroll } from './scrollState';
import { isElementVisible } from '../../utils/elementVisibility';
import { getVisiblePromptInput } from './promptFocus';
import { focusedPane } from '../../store/store';
import { focusIntoPane } from '../layout/paneFocus';


export const CHOICE_CARD_ROLE = 'card-choices';

const CHOICE_CARD_SELECTOR = `[data-role="${CHOICE_CARD_ROLE}"]`;
const CHOICE_SELECTOR = 'button:not([disabled]):not([data-not-choice])';

function arrowDelta(key: string): 1 | -1 | null {
  if (key === 'ArrowDown' || key === 'ArrowRight') return 1;
  if (key === 'ArrowUp' || key === 'ArrowLeft') return -1;
  return null;
}

function stepChoiceIndex(current: number, delta: 1 | -1, count: number): number {
  if (current < 0) return delta > 0 ? 0 : count - 1;
  return Math.max(0, Math.min(count - 1, current + delta));
}

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

export function shouldSeedChoiceFocus(opts: {
  hoverPointer: boolean;
  promptHasText: boolean;
  activeIsIdle: boolean;
  markerOnThreadPane: boolean;
}): boolean {
  return opts.hoverPointer && !opts.promptHasText && opts.activeIsIdle && opts.markerOnThreadPane;
}

function choiceButtons(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(CHOICE_SELECTOR));
}

function defaultChoice(root: HTMLElement): HTMLElement | null {
  const marked = root.querySelector<HTMLElement>('button[data-default-choice]:not([disabled])');
  return marked ?? choiceButtons(root)[0] ?? null;
}

function promptHasText(): boolean {
  const el = getVisiblePromptInput() as HTMLTextAreaElement | null;
  return (el?.value ?? '').trim().length > 0;
}

function activeElementIsIdle(): boolean {
  const active = document.activeElement as HTMLElement | null;
  if (!active || active === document.body || active === document.documentElement) return true;
  if (active.dataset?.role === 'prompt-input') return true;
  return active.classList?.contains('thread-content') === true;
}

export function handleChoiceCardKeyDown(e: KeyboardEvent, root: HTMLElement | null): void {
  if (!root) return;
  const items = choiceButtons(root);
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next = nextChoiceIndex(e, current, items.length);
  if (next === null) return;
  e.preventDefault();
  if (next === current) return;
  items[next].focus({ preventScroll: true });
  items[next].scrollIntoView({ block: 'nearest' });
  markRevealScroll();
}

export function handAnsweredCardFocusToPrompt(): void {
  const active = document.activeElement as HTMLElement | null;
  if (!hasHoverPointer() || !active?.closest(CHOICE_CARD_SELECTOR)) return;
  focusIntoPane(getVisiblePromptInput());
}

const seededCards = new Set<string>();

export function claimSeedForCard(cardId: string): boolean {
  if (seededCards.has(cardId)) return false;
  seededCards.add(cardId);
  return true;
}

export function seedChoiceCardFocus(root: HTMLElement | null, cardId: string): void {
  if (!root || !claimSeedForCard(cardId)) return;
  if (!shouldSeedChoiceFocus({
    hoverPointer: hasHoverPointer(),
    promptHasText: promptHasText(),
    activeIsIdle: activeElementIsIdle(),
    markerOnThreadPane: focusedPane.value === 'thread',
  })) return;
  const choice = defaultChoice(root);
  if (choice && isElementOnScreen(choice)) choice.focus({ preventScroll: true });
}

export function threadEntryFocusTarget(prompt: HTMLElement | null): HTMLElement | null {
  if (!hasHoverPointer() || promptHasText()) return prompt;
  const roots = Array.from(document.querySelectorAll<HTMLElement>(CHOICE_CARD_SELECTOR))
    .filter(isElementVisible);
  const last = roots[roots.length - 1];
  const choice = last ? defaultChoice(last) : null;
  return choice && isElementOnScreen(choice) ? choice : prompt;
}
