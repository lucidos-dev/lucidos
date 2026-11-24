import { isReducedMotion, scaledDurationMs } from '../../utils/motion';
import { MODAL_EXIT_DRAWING_CLASS } from './overlayExitClass';

export const MODAL_FADE_MS = 150;
const MODAL_FADE_SLACK_MS = 50;

const LIVE_CONTENT = 'iframe, video, audio, canvas, object, embed';

export const MAX_DRAWN_ELEMENTS = 5000;

const LOOKUP_ATTRS = ['id', 'data-role', 'data-overlay-panel', 'autofocus'];

export interface ExitLook {
  cls: string;
  style: string;
}

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
