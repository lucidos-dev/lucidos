
import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { isReducedMotion, scaledDurationMs } from '../../utils/motion';

const HEIGHT_EASE_MS = 300;
const HEIGHT_EASE_SLACK_MS = 100;

const cache = new WeakMap<HTMLTextAreaElement, { height: number; len: number }>();

function boxWidth(el: HTMLTextAreaElement): number {
  return el.getBoundingClientRect?.().width ?? 0;
}

function placeholderHeight(el: HTMLTextAreaElement): number {
  const parent = el.parentElement;
  if (!el.placeholder || el.value.length > 0 || !parent || !el.cloneNode) return 0;
  const probe = el.cloneNode() as HTMLTextAreaElement;
  probe.value = el.placeholder;
  probe.removeAttribute?.('data-role');
  Object.assign(probe.style, {
    position: 'absolute', visibility: 'hidden', boxSizing: 'border-box',
    width: `${boxWidth(el)}px`,
    height: '0', minHeight: '0', maxHeight: 'none',
  });
  parent.appendChild(probe);
  const needed = probe.scrollHeight;
  probe.remove();
  return needed;
}

function applyHeight(el: HTMLTextAreaElement, measured: number, prevHeight: number, curLen: number): boolean {
  const contentHeight = Math.max(measured, placeholderHeight(el));
  el.style.height = contentHeight + 'px';
  el.style.overflowY = 'hidden';
  const rendered = el.offsetHeight;
  cache.set(el, { height: rendered, len: curLen });
  if (rendered < contentHeight) {
    el.style.overflowY = 'auto';
  } else {
    el.scrollTop = 0;
  }
  return rendered !== prevHeight;
}

export function resizeTextarea(el: HTMLTextAreaElement): boolean {
  if (boxWidth(el) === 0) return false;
  const cached = cache.get(el);
  const prevHeight = cached?.height ?? 0;
  const prevLen = cached?.len ?? -1;
  const curLen = el.value.length;

  if (prevLen >= 0 && curLen - prevLen > 1) {
    abandonHeightAnimation(el);
    el.style.height = '0';
    return applyHeight(el, el.scrollHeight, prevHeight, curLen);
  }

  const scrollH = el.scrollHeight;
  const clientH = el.clientHeight;

  if (prevHeight > 0 && scrollH <= clientH && curLen >= prevLen) {
    if (cached) cached.len = curLen;
    return false;
  }

  abandonHeightAnimation(el);

  if (scrollH > clientH) {
    return applyHeight(el, scrollH, prevHeight, curLen);
  }

  el.style.height = '0';
  return applyHeight(el, el.scrollHeight, prevHeight, curLen);
}

export function remeasureTextarea(el: HTMLTextAreaElement): boolean {
  if (boxWidth(el) === 0) return false;
  const prevHeight = cache.get(el)?.height ?? 0;
  el.style.height = '0';
  return applyHeight(el, el.scrollHeight, prevHeight, el.value.length);
}

const pendingHeightAnim = new WeakMap<HTMLTextAreaElement, () => void>();

export function isTextareaHeightAnimating(el: HTMLTextAreaElement): boolean {
  return pendingHeightAnim.has(el);
}

function abandonHeightAnimation(el: HTMLTextAreaElement): void {
  pendingHeightAnim.get(el)?.();
}

function stopHeightTransition(el: HTMLTextAreaElement): void {
  for (const animation of el.getAnimations?.() ?? []) {
    if ((animation as CSSTransition).transitionProperty === 'height') animation.cancel();
  }
}

export function animateTextareaHeightFrom(el: HTMLTextAreaElement, fromHeight: string): void {
  pendingHeightAnim.get(el)?.();

  const target = el.style.height;
  if (!target || !fromHeight || target === fromHeight) return;

  el.style.transition = 'none';
  el.style.height = fromHeight;
  void el.offsetHeight;

  let timer: ReturnType<typeof setTimeout> | undefined;
  let raf1: number | undefined;
  let raf2: number | undefined;
  let started = false;
  const onHidden = () => {
    if (!started && document.visibilityState === 'hidden') finish();
  };
  const cancel = () => {
    if (timer !== undefined) clearTimeout(timer);
    if (raf1 !== undefined) cancelAnimationFrame(raf1);
    if (raf2 !== undefined) cancelAnimationFrame(raf2);
    el.removeEventListener('transitionend', finish);
    document.removeEventListener('visibilitychange', onHidden);
    el.style.transition = '';
    stopHeightTransition(el);
    pendingHeightAnim.delete(el);
  };
  const finish = (e?: TransitionEvent) => {
    if (e && (e.target !== el || e.propertyName !== 'height')) return;
    cancel();
    el.style.height = target;
  };
  pendingHeightAnim.set(el, cancel);
  document.addEventListener('visibilitychange', onHidden);

  raf1 = requestAnimationFrame(() => {
    raf2 = requestAnimationFrame(() => {
      started = true;
      el.addEventListener('transitionend', finish);
      el.style.transition = `height ${scaledDurationMs(HEIGHT_EASE_MS)}ms ease`;
      el.style.height = target;
      timer = setTimeout(finish, scaledDurationMs(HEIGHT_EASE_MS) + HEIGHT_EASE_SLACK_MS);
    });
  });
}

export function easeEmptiedTextarea(el: HTMLTextAreaElement): void {
  const fromHeight = el.style.height;
  remeasureTextarea(el);
  if (!isReducedMotion()) animateTextareaHeightFrom(el, fromHeight);
}

export function remeasureTextareaForPlaceholder(el: HTMLTextAreaElement): void {
  if (!isTextareaHeightAnimating(el)) {
    remeasureTextarea(el);
    return;
  }
  const current = getComputedStyle(el).height;
  abandonHeightAnimation(el);
  remeasureTextarea(el);
  animateTextareaHeightFrom(el, current);
}

function observeWidth(el: HTMLTextAreaElement): { el: HTMLTextAreaElement; stop: () => void } {
  let lastWidth = boxWidth(el);
  const observer = new ResizeObserver(() => {
    const width = boxWidth(el);
    if (width === lastWidth) return;
    lastWidth = width;
    abandonHeightAnimation(el);
    remeasureTextarea(el);
  });
  observer.observe(el);
  return { el, stop: () => observer.disconnect() };
}

export function useWidthRemeasure(ref: RefObject<HTMLTextAreaElement | null>) {
  const attached = useRef<{ el: HTMLTextAreaElement; stop: () => void } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (el === (attached.current?.el ?? null)) return;
    attached.current?.stop();
    attached.current = el ? observeWidth(el) : null;
  });

  useEffect(() => () => {
    attached.current?.stop();
    attached.current = null;
  }, []);
}

export function useFontMetricsResize(onResize: () => void) {
  useEffect(() => {
    const root = document.documentElement;
    let lastScale = root.style.getPropertyValue('--user-ui-scale');
    let lastFont = root.style.getPropertyValue('--font-ui');
    let rafId: number | null = null;

    const observer = new MutationObserver(() => {
      const scale = root.style.getPropertyValue('--user-ui-scale');
      const font = root.style.getPropertyValue('--font-ui');
      if (scale !== lastScale || font !== lastFont) {
        lastScale = scale;
        lastFont = font;
        if (rafId === null) {
          rafId = requestAnimationFrame(() => { rafId = null; onResize(); });
        }
      }
    });
    observer.observe(root, { attributes: true, attributeFilter: ['style'] });

    document.fonts.addEventListener('loadingdone', onResize);

    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      observer.disconnect();
      document.fonts.removeEventListener('loadingdone', onResize);
    };
  }, []);
}
