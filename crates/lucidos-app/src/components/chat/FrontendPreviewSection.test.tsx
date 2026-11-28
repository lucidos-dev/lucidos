// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FrontendPreviewSection } from './FrontendPreviewSection';
import { frontendPreview } from '../../store/actions/frontend-preview';
import { isTauri } from '../../utils/platform';

vi.mock('../../utils/platform', () => ({ isTauri: vi.fn() }));

const host = document.createElement('div');
afterEach(() => {
  act(() => render(null, host));
  frontendPreview.value = { status: 'not-loaded' };
});

it.each([false, true])('opens on the reached origin in desktop=%s', (desktop) => {
  vi.mocked(isTauri).mockReturnValue(desktop);
  const path = '/~preview/0123456789abcdef/demo/';
  frontendPreview.value = { status: 'loaded', data: { running: true, thread_id: 'example-thread', path } };
  act(() => render(<FrontendPreviewSection threadId="example-thread" />, host));
  const link = host.querySelector('a')!;
  expect(link.href).toBe(`${location.origin}${path}#thread=example-thread`);
  expect(link.target).toBe(desktop ? '_self' : '_blank');
});
