// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { EngineBuildLogModal } from './EngineBuildLogModal';
import { engineBuildLogOpen, engineBuildLogVersion } from '../../store/engineBuildLog';
import { repairGapState } from '../../store/eventGap';
import { API } from '../../utils/basePath';

let host: HTMLDivElement;
const fetchLog = vi.fn();
beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  engineBuildLogOpen.value = true;
  vi.stubGlobal('fetch', fetchLog);
  fetchLog.mockReset();
});
afterEach(() => {
  act(() => render(null, host));
  host.remove();
  engineBuildLogOpen.value = false;
  vi.unstubAllGlobals();
});
it('fetches with the paired window and renders log text literally', async () => {
  const output = '<script>untrusted</script>\ncompile failed';
  fetchLog.mockResolvedValue(new Response(output, { status: 200 }));
  act(() => render(<EngineBuildLogModal />, host));
  await vi.waitFor(() => expect(document.querySelector('[data-role="engine-build-log"] pre')?.textContent).toBe(output));
  expect(fetchLog).toHaveBeenCalledWith(`${API}/engine/build-log`, expect.objectContaining({ credentials: 'same-origin' }));
  expect(document.querySelector('[data-role="engine-build-log"] script')).toBeNull();
  act(() => (document.querySelector('[aria-label="Close build log"]') as HTMLButtonElement).click());
  expect(engineBuildLogOpen.value).toBe(false);
});
it('shows a readable error and retries within the same window', async () => {
  fetchLog.mockResolvedValueOnce(new Response('Unavailable', { status: 404 }))
    .mockResolvedValueOnce(new Response('retry output', { status: 200 }));
  act(() => render(<EngineBuildLogModal />, host));
  await vi.waitFor(() => expect(document.querySelector('[data-role="engine-build-log"]')?.textContent).toContain('Failed to load build output'));
  const retry = [...document.querySelectorAll<HTMLButtonElement>('[data-role="engine-build-log"] button')].find(button => button.textContent === 'Retry')!;
  act(() => retry.click());
  await vi.waitFor(() => expect(document.querySelector('[data-role="engine-build-log"] pre')?.textContent).toBe('retry output'));
});
it.each([
  ['build progress', () => { engineBuildLogVersion.value++; }],
  ['an event gap', repairGapState],
] as const)('keeps the visible log while refreshing after %s', async (_, refresh) => {
  let finish!: (response: Response) => void;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  fetchLog.mockResolvedValueOnce(new Response('compiling', { status: 200 }))
    .mockReturnValueOnce(pending);
  act(() => render(<EngineBuildLogModal />, host));
  await vi.waitFor(() => expect(document.querySelector('[data-role="engine-build-log"] pre')?.textContent).toBe('compiling'));
  const visible = document.querySelector<HTMLElement>('[data-role="engine-build-log"] pre')!;
  visible.scrollTop = 40;
  act(refresh);
  await vi.waitFor(() => expect(fetchLog).toHaveBeenCalledTimes(2));
  expect(visible.isConnected).toBe(true);
  expect(visible.textContent).toBe('compiling');
  expect(visible.scrollTop).toBe(40);
  finish(new Response('publish failed', { status: 200 }));
  await vi.waitFor(() => expect(document.querySelector('[data-role="engine-build-log"] pre')?.textContent).toBe('publish failed'));
});
