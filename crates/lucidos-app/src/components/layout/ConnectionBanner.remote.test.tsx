// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
vi.mock('../../utils/basePath', async () => ({
  ...await vi.importActual('../../utils/basePath'), WORKSPACE_ID: 'dev@cloud', REMOTE_GATEWAY: 'cloud',
}));
import { remoteConnectionFailure } from '../../utils/remoteConnection';
import { viewportIsMobile } from '../../utils/viewport';
import { ConnectionBanner } from './ConnectionBanner';
it('offers the picker instead of a local Start for an unreachable remote', () => {
  viewportIsMobile.value = false;
  remoteConnectionFailure.value = { reason: 'Remote timed out', reason_kind: 'timeout' };
  const container = document.createElement('div');
  act(() => render(<ConnectionBanner layout="desktop" />, container));
  expect(container.textContent).toContain('Cannot reach @cloud');
  expect(container.querySelector('a')?.getAttribute('href')).toBe('/~/?pick');
  expect(container.querySelector('button')).toBeNull();
  act(() => render(null, container));
  remoteConnectionFailure.value = null;
});
