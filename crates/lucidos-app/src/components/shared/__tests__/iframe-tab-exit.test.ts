// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { onIframeTabExitFocus } from '../IframeTabExit';

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

  it.each(['missing', 'the frame'] as const)('hands a Tab out when the related target is %s', (target) => {
    const { frame, stop, next } = pane();
    onIframeTabExitFocus({ currentTarget: stop, relatedTarget: target === 'missing' ? null : frame });
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
