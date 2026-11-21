// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { BrandMenuButton } from './HeaderMark';
import { engineBuilding, engineBuildDetail } from '../../store/store';

let host: HTMLDivElement | undefined;
afterEach(() => {
  if (host) { render(null, host); host.remove(); host = undefined; }
  engineBuilding.value = false;
  engineBuildDetail.value = null;
  vi.useRealTimers();
});

it('advances the desktop build tooltip without another status read', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(100_000);
  engineBuilding.value = true;
  engineBuildDetail.value = { elapsedMs: 5_000, anchoredAt: Date.now(), pendingCommits: null };
  host = document.createElement('div');
  document.body.append(host);
  await act(async () => { render(<BrandMenuButton placement="brand" />, host!); });
  const tooltip = () => host!.querySelector('button')!.getAttribute('data-tooltip-rows');
  expect(tooltip()).toContain('5s');
  await act(async () => { vi.advanceTimersByTime(2_000); });
  expect(tooltip()).toContain('7s');
});
