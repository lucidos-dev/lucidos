import { EASING_DISCLOSURE } from '../utils/disclosureMotion';
import { isReducedMotion, scaledDurationMs } from '@lucidos/motion';

export const SURFACE_STEP_ATTR = 'data-surface-step';

const STEP_MORPH_MS = 220;
const STEP_MORPH_SLACK_MS = 100;

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

export function boxOf(el: Element): Box {
  const { left, top, width, height } = el.getBoundingClientRect();
  return { left, top, width, height };
}

export function stepSignature(panel: Element): string {
  return Array.from(panel.querySelectorAll(`[${SURFACE_STEP_ATTR}]`))
    .map((el) => el.getAttribute(SURFACE_STEP_ATTR))
    .join('\n');
}

export function morphKeyframes(from: Box, to: Box): Keyframe[] {
  return [
    {
      width: `${from.width}px`,
      height: `${from.height}px`,
      translate: `${from.left - to.left}px ${from.top - to.top}px`,
    },
    { width: `${to.width}px`, height: `${to.height}px`, translate: '0px 0px' },
  ];
}

function pinStyle(el: HTMLElement, props: Record<string, string>): () => void {
  const saved = Object.keys(props).map((p) => [p, el.style.getPropertyValue(p), el.style.getPropertyPriority(p)]);
  for (const [p, v] of Object.entries(props)) el.style.setProperty(p, v);
  return () => {
    for (const [p, v, priority] of saved) {
      if (v) el.style.setProperty(p, v, priority);
      else el.style.removeProperty(p);
    }
  };
}

const heldUntilSettled = new WeakMap<Element, (() => void)[]>();

export function whenStepSettled(el: Element | null | undefined, fn: () => void): void {
  for (let node = el ?? null; node; node = node.parentElement) {
    const held = heldUntilSettled.get(node);
    if (held) {
      held.push(fn);
      return;
    }
  }
  fn();
}

export interface StepMorph {
  cancel(): void;
}

export function startStepMorph(panel: HTMLElement, from: Box, onSettled: () => void): StepMorph | null {
  if (isReducedMotion() || typeof panel.animate !== 'function') return null;
  const to = boxOf(panel);
  const children = Array.from(panel.children).filter((c): c is HTMLElement => c instanceof HTMLElement);
  const sizes = children.map(boxOf);
  const unpins = [
    pinStyle(panel, { overflow: 'hidden' }),
    ...children.map((child, i) => pinStyle(child, {
      width: `${sizes[i].width}px`,
      height: `${sizes[i].height}px`,
      'flex-grow': '0',
      'flex-shrink': '0',
    })),
  ];
  const duration = scaledDurationMs(STEP_MORPH_MS);
  const animations = [
    panel.animate(morphKeyframes(from, to), { duration, easing: EASING_DISCLOSURE }),
    ...children.map((child) => child.animate([{ opacity: 0 }, { opacity: 1 }], { duration, easing: 'ease-out' })),
  ];
  const held: (() => void)[] = [];
  heldUntilSettled.set(panel, held);
  let done = false;
  const stop = () => {
    if (done) return false;
    done = true;
    clearTimeout(safety);
    for (const a of animations) a.cancel();
    for (const unpin of unpins) unpin();
    heldUntilSettled.delete(panel);
    for (const fn of held) fn();
    return true;
  };
  const finish = () => {
    if (stop()) onSettled();
  };
  animations[0].onfinish = finish;
  const safety = setTimeout(finish, scaledDurationMs(STEP_MORPH_MS) + STEP_MORPH_SLACK_MS);
  return { cancel: () => { stop(); } };
}
