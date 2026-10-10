/**
 * The *step morph*: an anchored popover gliding from one *surface step* to the
 * next, instead of jumping (`docs/glossary.md`).
 *
 * The panel's box animates from its old rect to its new one, and the new
 * content fades in. The children hold their final size throughout, and the
 * panel clips them. So no text rewraps frame by frame and no scrollbar flashes.
 * `useAnchoredPosition` decides when a step changed and stops re-measuring
 * while a morph runs.
 */
import { EASING_DISCLOSURE } from '../utils/disclosureMotion';
import { isReducedMotion, scaledDurationMs } from '../utils/motion';

/** Carried by the root of each step's content, naming the step. */
export const SURFACE_STEP_ATTR = 'data-surface-step';

const STEP_MORPH_MS = 220;
/** A fixed margin past the morph before the safety timer settles it. */
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

/** Every step the panel shows, outermost first. Steps nest: the model picker's
 *  tier step sits inside the coding-agent menu's model step. Empty when the
 *  panel declares no step. */
export function stepSignature(panel: Element): string {
  return Array.from(panel.querySelectorAll(`[${SURFACE_STEP_ATTR}]`))
    .map((el) => el.getAttribute(SURFACE_STEP_ATTR))
    .join('\n');
}

/** The panel's box from `from` to `to`, drawn at `to`'s position. Size and
 *  offset share one curve, so an edge both boxes share stays put: an upward
 *  panel keeps its bottom on its anchor. */
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

/** Set inline properties on `el`, returning what puts back the old values. */
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

/** Work held until a panel's running morph ends, keyed by panel. */
const heldUntilSettled = new WeakMap<Element, (() => void)[]>();

/** Run `fn` once the step morph around `el` ends, or now when none runs.
 *  Anything that measures the panel's box waits: mid-morph that box is the
 *  animated one. A `scrollIntoView` then scrolls for a height the panel is
 *  about to leave. */
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
  /** Stop at once and leave nothing behind. Does not call `onSettled`. */
  cancel(): void;
}

/** Glide `panel` from `from` to the box it has now. Call it once the new step
 *  is laid out at its final position, before the frame paints.
 *
 *  Returns null when there is nothing to run: reduced motion, or no Web
 *  Animations. The swap is then instant. */
export function startStepMorph(panel: HTMLElement, from: Box, onSettled: () => void): StepMorph | null {
  if (isReducedMotion() || typeof panel.animate !== 'function') return null;
  const to = boxOf(panel);
  const children = Array.from(panel.children).filter((c): c is HTMLElement => c instanceof HTMLElement);
  // Measure every child before pinning any, since a pin can reflow its siblings.
  const sizes = children.map(boxOf);
  const unpins = [
    pinStyle(panel, { overflow: 'hidden' }),
    ...children.map((child, i) => pinStyle(child, {
      width: `${sizes[i].width}px`,
      height: `${sizes[i].height}px`,
      // Longhands, since the shorthand does not round-trip through a restore.
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
  // A hidden page can hold the finish event back, and the panel would stay
  // pinned with its position frozen until then.
  const safety = setTimeout(finish, scaledDurationMs(STEP_MORPH_MS) + STEP_MORPH_SLACK_MS);
  return { cancel: () => { stop(); } };
}
