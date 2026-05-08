import { splitRatio } from '../../store/store';

export const DEFAULT_SPLIT_RATIO = 0.4;

/** Animate the regular brand back when expanding from collapsed */
export function animateBrandReturn(targetRatio: number) {
  const header = document.querySelector('.app-header') as HTMLElement | null;
  if (!header) return;

  const hRect = header.getBoundingClientRect();

  const mergedCenter = hRect.left + hRect.width / 2;
  const futureBrandCenter = hRect.left + (targetRatio / 2) * hRect.width;

  const travel = mergedCenter - futureBrandCenter;
  document.documentElement.style.setProperty('--brand-return-x', `${travel}px`);

  document.documentElement.removeAttribute('data-brand-returning');
  void document.documentElement.offsetWidth;
  document.documentElement.setAttribute('data-brand-returning', '');

  const brand = document.querySelector('.app-header .pane-header-brand');
  brand?.addEventListener('animationend', () => {
    document.documentElement.removeAttribute('data-brand-returning');
  }, { once: true });
}

/** Add a brief CSS transition for snap animations */
export function triggerSnapAnimate() {
  const container = document.querySelector('.split-layout') as HTMLElement | null;
  if (!container) return;
  container.classList.add('snap-animate');
  setTimeout(() => container.classList.remove('snap-animate'), 300);
}

/** Update splitRatio with animations and persist to localStorage */
export function setSplitRatio(newRatio: number) {
  const oldRatio = splitRatio.value;

  // Expanding from chat-collapsed → animate brand returning
  if (oldRatio === 0 && newRatio > 0) animateBrandReturn(newRatio);

  triggerSnapAnimate();
  splitRatio.value = newRatio;
  localStorage.setItem('lucidos-split-ratio', String(newRatio));
}
