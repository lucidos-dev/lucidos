// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { onIframeTabExitFocus } from '../IframeTabExit';

// Keys pressed inside an app or preview frame never reach the host. Tab past
// the frame's last control therefore left the content pane for whatever came
// next in the document: a toast, the browser chrome, or the header.
describe('onIframeTabExitFocus', () => {
  afterEach(() => { document.body.innerHTML = ''; });

  function pane(after = true): { frame: HTMLElement; stop: HTMLElement; first: HTMLElement; next: HTMLElement | null } {
    document.body.innerHTML = `
      <div class="pane-content">
        <button data-id="first">Back</button>
        <iframe></iframe><span tabindex="0" data-id="stop"></span>
        ${after ? '<button data-id="next">Open</button>' : ''}
      </div>
      <div class="toast-container"><button>Dismiss</button></div>`;
    for (const el of document.querySelectorAll<HTMLElement>('*')) {
      el.getClientRects = () => [{}] as unknown as DOMRectList;
    }
    const q = (id: string) => document.querySelector<HTMLElement>(`[data-id="${id}"]`);
    return { frame: document.querySelector('iframe')!, stop: q('stop')!, first: q('first')!, next: q('next') };
  }

  it('hands a Tab out of the frame to the next control in the pane', () => {
    const { stop, next } = pane();
    onIframeTabExitFocus({ currentTarget: stop, relatedTarget: null });
    expect(document.activeElement).toBe(next);
  });

  it('wraps to the pane\'s first control when the frame is last', () => {
    const { stop, first } = pane(false);
    onIframeTabExitFocus({ currentTarget: stop, relatedTarget: null });
    expect(document.activeElement).toBe(first);
  });

  it('sends a Shift+Tab from below back into the frame', () => {
    const { stop, frame, next } = pane();
    onIframeTabExitFocus({ currentTarget: stop, relatedTarget: next });
    expect(document.activeElement).toBe(frame);
  });
});
