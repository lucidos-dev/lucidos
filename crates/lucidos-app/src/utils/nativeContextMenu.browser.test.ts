// @vitest-environment jsdom
// Its own file: the desktop suite installs document listeners that outlive it.
import { describe, it, expect } from 'vitest';
import { installNativeContextMenuPolicy } from './nativeContextMenu';

describe('installNativeContextMenuPolicy in a browser tab', () => {
  it('never cancels a native menu, since the browser owns it', () => {
    installNativeContextMenuPolicy();
    const chrome = document.createElement('span');
    document.body.appendChild(chrome);
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    chrome.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });
});
