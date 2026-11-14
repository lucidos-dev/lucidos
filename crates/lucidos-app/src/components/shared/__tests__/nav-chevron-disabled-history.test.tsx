// @vitest-environment jsdom
/**
 * A disabled chevron opens no history menu.
 *
 * It takes pointer events so its tooltip shows (`.nav-chevron:disabled` in
 * global/host-components.css). Chromium and WebKit then deliver `pointerdown`
 * and `contextmenu` to it, but never `click`. So a hold or a right-click
 * opened a "No history" menu that a second tap on the chevron could not close.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { NavChevron } from '../NavChevron';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('a disabled nav chevron', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  async function rightClick(disabled: boolean): Promise<Element | null> {
    render(
      <NavChevron direction="back" disabled={disabled} onStep={() => {}} getItems={() => []} ariaLabel="Back" />,
      host,
    );
    host.querySelector('button')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
    await settled();
    return document.querySelector('.nav-history-menu');
  }

  it('opens no history menu on right-click', async () => {
    expect(await rightClick(true)).toBeNull();
  });

  it('an enabled one still does', async () => {
    expect(await rightClick(false)).not.toBeNull();
  });
});
