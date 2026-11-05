// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
vi.mock('../../utils/basePath', async () => ({
  ...await vi.importActual('../../utils/basePath'),
  WORKSPACE_ID: 'dev@cloud',
  REMOTE_GATEWAY: 'cloud',
}));
import { workspaceName } from '../../store/store';
import { WorkspaceNameLabel } from './WorkspaceNameLabel';
it('identifies the remote gateway in the header inside dev@cloud', () => {
  workspaceName.value = 'dev';
  const container = document.createElement('div');
  act(() => render(<WorkspaceNameLabel />, container));
  expect(container.textContent).toContain('dev @cloud');
  expect(container.textContent).toContain('remote');
});
