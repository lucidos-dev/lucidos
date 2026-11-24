import { scaledDurationMs } from './motion';
import { startIdlePrefetch } from './idlePrefetch';
import { markBoot } from './bootTiming';

const SPLASH_SELECTOR = '.boot-splash';
const STATUS_SELECTOR = '.boot-splash-status';
const LEAVING_CLASS = 'boot-splash-leaving';
const SWAP_CLASS = 'boot-splash-status-swap';
export const STATUS_SWAP_MS = 150;
const FORMED_CLASS_SELECTOR = '.boot-splash-formed';
// Set on <html> by the quiet script in index.html's <head>. It marks a document
// whose load CONTINUES a session rather than starting one, so it carries no mark.
// Two triggers: a user-requested refresh (permanent) and a notification-tap
// deep link (whose URL form is the temporary half, tracked at
// docs/temporary-measures.md § "Cross-document notification-tap reload on iOS").
// The cover itself is permanent either way: do NOT delete it with that measure.
const QUIET_ATTRIBUTE = 'data-boot-splash-quiet';

const VEIL_FADE_MS = 650;
const FADE_REMOVE_SLACK_MS = 250;

let dismissed = false;

export function bootSplashPresent(): boolean {
  return !dismissed && document.querySelector(SPLASH_SELECTOR) !== null;
}

export function bootSplashPlaysNoReveal(): boolean {
  return (
    document.querySelector(SPLASH_SELECTOR + FORMED_CLASS_SELECTOR) !== null ||
    document.documentElement.hasAttribute(QUIET_ATTRIBUTE)
  );
}

export function statusChangeFades(shown: string, next: string): boolean {
  if (shown === '' || next === '') return false;
  const withoutCounters = (text: string) => text.replace(/\d+/g, '0');
  return withoutCounters(shown) !== withoutCounters(next);
}

let pendingSwap: number | undefined;

const ACTION_SELECTOR = '.boot-splash-action';

export interface BootAction {
  label: string;
  onPress: () => void;
}

export function setBootAction(action: BootAction | null): void {
  const splash = document.querySelector(SPLASH_SELECTOR);
  const status = splash?.querySelector(STATUS_SELECTOR);
  if (!splash || !status) return;
  const shown = splash.querySelector(ACTION_SELECTOR);
  if (shown && shown.textContent === action?.label) return;
  shown?.remove();
  if (!action) return;
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'boot-splash-action';
  button.textContent = action.label;
  button.addEventListener('click', action.onPress);
  status.after(button);
  splash.setAttribute('aria-hidden', 'false');
}

export function setBootStatus(text: string): void {
  const el = document.querySelector(SPLASH_SELECTOR + ' ' + STATUS_SELECTOR);
  if (!el) return;
  window.clearTimeout(pendingSwap);
  const write = () => {
    el.textContent = text;
    el.classList.toggle(SWAP_CLASS, false);
    el.classList.toggle('boot-splash-status-shown', text.length > 0);
    el.classList.toggle('boot-splash-status-report', text.includes('\n'));
  };
  if (!statusChangeFades(el.textContent ?? '', text)) {
    write();
    return;
  }
  el.classList.toggle(SWAP_CLASS, true);
  pendingSwap = window.setTimeout(write, scaledDurationMs(STATUS_SWAP_MS));
}

export function revealBootEscape(): boolean {
  const reveal = (window as Window & { __lucidosGatewayEscape?: () => unknown })
    .__lucidosGatewayEscape;
  if (typeof reveal !== 'function') return false;
  return reveal() != null;
}

export function handOverBootOwnership(): void {
  (window as Window & { __lucidosBootLoaded?: () => void }).__lucidosBootLoaded?.();
}

export function dismissBootSplash(): void {
  if (dismissed) return;
  dismissed = true;
  const canvases = [document.documentElement, document.body].filter(Boolean);
  const lifted = () => {
    markBoot('splashLifted');
    for (const canvas of canvases) {
      canvas.style.background = '';
      canvas.style.transition = '';
    }
    startIdlePrefetch();
  };
  const el = document.querySelector(SPLASH_SELECTOR);
  if (!el) {
    lifted();
    return;
  }
  el.classList.add(LEAVING_CLASS);
  const veil = getComputedStyle(el);
  for (const canvas of canvases) {
    canvas.style.transition = `background-color ${veil.animationDuration} ${veil.animationTimingFunction}`;
    canvas.style.background = 'var(--bg-primary)';
  }
  let removed = false;
  const remove = () => {
    if (removed) return;
    removed = true;
    el.remove();
    lifted();
  };
  const onAnimationEnd = (event: Event) => {
    if (event.target !== el) return;
    el.removeEventListener('animationend', onAnimationEnd);
    remove();
  };
  el.addEventListener('animationend', onAnimationEnd);
  window.setTimeout(remove, scaledDurationMs(VEIL_FADE_MS) + FADE_REMOVE_SLACK_MS);
}
