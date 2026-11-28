// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { UiBlockingOverlay } from '../UiBlockingOverlay';
import { clientRefreshing } from '../../../hooks/sw-update';

let host: HTMLDivElement;
let field: HTMLInputElement;

beforeEach(async () => {
  clientRefreshing.value = false;
  host = document.createElement('div');
  document.body.appendChild(host);
  field = document.createElement('input');
  document.body.appendChild(field);
  await act(() => { render(<UiBlockingOverlay />, host); });
});

afterEach(async () => {
  await act(() => { clientRefreshing.value = false; });
  await act(() => { render(null, host); });
  host.remove();
  field.remove();
});

describe('UiBlockingOverlay while a refresh holds the page', () => {
  it('flags the document while the block is up, and clears the flag after', async () => {
    await act(() => { clientRefreshing.value = true; });
    expect(document.documentElement.hasAttribute('data-ui-blocked')).toBe(true);
    await act(() => { clientRefreshing.value = false; });
    expect(document.documentElement.hasAttribute('data-ui-blocked')).toBe(false);
  });

  it('stops a key before it reaches the field it was pressed in', async () => {
    await act(() => { clientRefreshing.value = true; });
    let reached = false;
    field.addEventListener('keydown', () => { reached = true; });
    field.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true }));
    expect(reached).toBe(false);
  });
});
