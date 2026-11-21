// @vitest-environment jsdom
/** The cache keep-alive setting covers the Lucidos Agent too, so it lives on
 *  Settings › Models (ADR 0556). Search lands there, and Coding Agents keeps a
 *  one-line pointer that opens it. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const { openCacheKeepAliveSetting } = vi.hoisted(() => ({ openCacheKeepAliveSetting: vi.fn() }));
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openCacheKeepAliveSetting,
}));

import { findSettingsEntry, settingsSearchCovers } from '../../search/searchIndex';
import { CacheKeepAlivePointer } from '../CacheKeepAliveSection';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  openCacheKeepAliveSetting.mockClear();
});

it('finds the setting under Models, never Coding Agents', () => {
  const entry = findSettingsEntry('models:cache-keep-alive');
  expect(entry?.subview).toBe('models');
  expect(entry?.path).toBe('Settings → Models');
  expect(settingsSearchCovers('coding-agents:cache-keep-alive')).toBe(false);
});

it('points Coding Agents at the setting on Models', async () => {
  await act(() => {
    render(<CacheKeepAlivePointer />, host);
  });
  const button = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Open Models');
  expect(button).toBeDefined();
  await act(() => {
    button!.click();
  });
  expect(openCacheKeepAliveSetting).toHaveBeenCalledTimes(1);
});
