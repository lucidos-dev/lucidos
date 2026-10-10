import { isReducedMotion, scaledDurationMs } from '../../utils/motion';
import { MODAL_EXIT_DRAWING_CLASS } from './overlayExitClass';

/** `--duration-fast`, the length of `modal-in`, which the drawing plays backwards. */
export const MODAL_FADE_MS = 150;
/** Keeps the drawing until its fade has surely ended. A fixed margin, so unscaled. */
const MODAL_FADE_SLACK_MS = 50;

/** A copy of these would load or play again rather than hold its last frame. */
const LIVE_CONTENT = 'iframe, video, audio, canvas, object, embed';

/** Past this size the copy itself is a visible hitch at close: about 50ms per
 *  20,000 elements in desktop Chromium. Such a modal closes at once instead. */
export const MAX_DRAWN_ELEMENTS = 5000;

/** Attributes a query could match the copy by, as if it were the live dialog. */
const LOOKUP_ATTRS = ['id', 'data-role', 'data-overlay-panel', 'autofocus'];

/** The backdrop container as it was last drawn open. */
export interface ExitLook {
  cls: string;
  style: string;
}

/**
 * Leave an inert copy of a closing backdrop modal in its place, fading out.
 *
 * Call it while `panel` is still attached and unchanged. The copy is a frozen
 * drawing: nothing in it answers a pointer, a key or a query. So the overlay
 * itself closes at once, and the stack, focus return and dismiss-swallow never
 * see the exit. A copy keeps typed text but drops scroll offsets and a
 * select's pick, so those two are carried over.
 */
export function leaveExitDrawing(panel: HTMLElement, look: ExitLook): void {
  const container = panel.parentElement;
  if (!container?.isConnected || isReducedMotion()) return;
  if (panel.getElementsByTagName('*').length > MAX_DRAWN_ELEMENTS) return;

  const copy = panel.cloneNode(true) as HTMLElement;
  const live = [panel, ...panel.querySelectorAll<HTMLElement>('*')];
  const copied = [copy, ...copy.querySelectorAll<HTMLElement>('*')];
  const scrolled: Array<[HTMLElement, number, number]> = [];
  live.forEach((el, i) => {
    const twin = copied[i];
    for (const attr of LOOKUP_ATTRS) twin.removeAttribute(attr);
    if (el.scrollTop || el.scrollLeft) scrolled.push([twin, el.scrollTop, el.scrollLeft]);
    if (el instanceof HTMLSelectElement) {
      (twin as HTMLSelectElement).value = el.value;
    } else if (el.matches(LIVE_CONTENT)) {
      twin.replaceWith(blankOfSameBox(el));
    }
  });

  const drawing = document.createElement('div');
  drawing.className = `${look.cls} ${MODAL_EXIT_DRAWING_CLASS}`;
  drawing.setAttribute('style', look.style);
  drawing.setAttribute('inert', '');
  drawing.setAttribute('aria-hidden', 'true');
  drawing.append(copy);
  // Before the live container, so a modal opening in the same slot paints
  // over the fading one rather than under it.
  container.before(drawing);
  for (const [el, top, left] of scrolled) {
    el.scrollTop = top;
    el.scrollLeft = left;
  }
  setTimeout(() => drawing.remove(), scaledDurationMs(MODAL_FADE_MS) + MODAL_FADE_SLACK_MS);
}

function blankOfSameBox(el: HTMLElement): HTMLElement {
  const blank = document.createElement('div');
  blank.className = el.getAttribute('class') ?? '';
  blank.style.cssText = el.style.cssText;
  const display = getComputedStyle(el).display;
  const box = el.getBoundingClientRect();
  blank.style.display = display === 'inline' ? 'inline-block' : display;
  blank.style.width = `${box.width}px`;
  blank.style.height = `${box.height}px`;
  return blank;
}
