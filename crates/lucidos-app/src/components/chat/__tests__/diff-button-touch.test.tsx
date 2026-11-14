// @vitest-environment jsdom
/**
 * Diff answers a touch, exactly once, and drops the keyboard itself.
 *
 * It sits in the composer's action row, so the user reaches it with the mobile
 * keyboard up. There WebKit drops the synthetic click, and the button was
 * reported dead in exactly that state, alongside the answer Submit and the lone
 * Cancel. Diff is non-destructive and idempotent, so it takes the touch path
 * that Send and the answer Submit already have.
 *
 * Rendered rather than poked through its props, because both promises live in
 * the composition: the twin window is per-instance state inside
 * `useTouchActivated`, and the blur is what replaces the shared listener on
 * `click` that a suppressed click never reaches.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { DiffButton } from '../WaitingBanner';

vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { viewThreadCcDiff } from '../../../store/actions/repositories';

let host: HTMLDivElement;
let prompt: HTMLTextAreaElement;

beforeEach(() => {
  vi.mocked(viewThreadCcDiff).mockReset();
  prompt = document.createElement('textarea');
  prompt.dataset.role = 'prompt-input';
  document.body.appendChild(prompt);
  host = document.createElement('div');
  document.body.appendChild(host);
  render(<DiffButton threadId="tid" />, host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  prompt.remove();
});

function button(): HTMLButtonElement {
  const btn = host.querySelector('button');
  expect(btn, 'DiffButton rendered no button').not.toBeNull();
  return btn as HTMLButtonElement;
}

/** A tap as WebKit delivers it: `touchend` first, then the synthetic click the
 *  touch path cancelled. jsdom dispatches both, so the twin window is what has
 *  to stop the second from running the action again. */
function tap(): void {
  const btn = button();
  btn.dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
  btn.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

describe('DiffButton', () => {
  it('opens the thread diff on a click, the desktop path', () => {
    button().click();
    expect(viewThreadCcDiff).toHaveBeenCalledTimes(1);
    expect(viewThreadCcDiff).toHaveBeenCalledWith('tid');
  });

  it('opens it on a touch, which is the only path with the keyboard up', () => {
    button().dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
    expect(viewThreadCcDiff).toHaveBeenCalledTimes(1);
  });

  it('runs once per tap, never twice on the touch and its twin click', () => {
    tap();
    expect(viewThreadCcDiff).toHaveBeenCalledTimes(1);
  });

  it('serves a genuine second tap, rather than eating it as a twin', () => {
    tap();
    tap();
    expect(viewThreadCcDiff).toHaveBeenCalledTimes(2);
  });

  it('cancels the synthetic click, so nothing downstream sees it', () => {
    // `installActionBtnBlurListener` and the app's other click consumers must
    // not act on a press the touch path already served.
    const btn = button();
    const touchend = new Event('touchend', { bubbles: true, cancelable: true });
    btn.dispatchEvent(touchend);
    expect(touchend.defaultPrevented).toBe(true);
  });

  it('drops the keyboard itself, since the suppressed click cannot', () => {
    prompt.focus();
    expect(document.activeElement).toBe(prompt);
    button().dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
    expect(document.activeElement).not.toBe(prompt);
  });

  it('leaves a focused field elsewhere alone', () => {
    // The blur is scoped to the prompt. A search box or a title editor holding
    // focus is not the composer's keyboard to drop.
    const other = document.createElement('input');
    document.body.appendChild(other);
    other.focus();
    button().dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(other);
    other.remove();
  });
});
