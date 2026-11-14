import { afterEach, expect, it, vi } from 'vitest';
import { postClientLog } from './clientLog';
import { observeSourceGeneration } from './restoreSourceGeneration';

afterEach(() => { observeSourceGeneration(null); vi.unstubAllGlobals(); });

it('stamps the originating page generation before serializing diagnostic data', () => {
  const fetchMock = vi.fn().mockResolvedValue({});
  vi.stubGlobal('fetch', fetchMock);
  observeSourceGeneration('original');
  postClientLog('test', 'breadcrumb', {
    toJSON() {
      observeSourceGeneration('restored');
      return { detail: 'captured before restore' };
    },
  });
  expect(new Headers(fetchMock.mock.calls[0][1].headers)
    .get('x-lucidos-restore-generation')).toBe('original');
  expect(fetchMock.mock.calls[0][1].keepalive).toBe(true);
});

it('explicitly stamps the legacy generation', () => {
  const fetchMock = vi.fn().mockResolvedValue({});
  vi.stubGlobal('fetch', fetchMock);
  postClientLog('test', 'breadcrumb', {});
  expect(new Headers(fetchMock.mock.calls[0][1].headers)
    .get('x-lucidos-restore-generation')).toBe('none');
});
