// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
vi.mock('../../utils/basePath', async () => ({
  ...await vi.importActual('../../utils/basePath'), WORKSPACE_ID: 'local', gatewayPickerHref: () => '/~/?pick',
}));
vi.mock('../../api/client/control', async () => ({
  ...await vi.importActual('../../api/client/control'),
  listWorkspacesWithOrder: vi.fn().mockResolvedValue({ workspaces: [{ id: 'local', name: 'Local workspace', health: 'healthy', port: 5000, autostart: false }], order: [] }),
  listRemoteWorkspaces: vi.fn(() => new Promise(() => {})),
}));
vi.mock('../shared/Disclosure', () => import('../shared/__tests__/disclosureStub'));
import { WorkspacesMenuRow } from './WorkspaceSwitcher';
import { rememberWorkspaceSwitcherExpanded } from '../../utils/lastWorkspace';
it('renders local rows and management while remote discovery never resolves', async () => {
  rememberWorkspaceSwitcherExpanded(true);
  const container = document.createElement('div');
  await act(async () => { render(<WorkspacesMenuRow onClose={() => {}} />, container); });
  await vi.waitFor(() => {
    expect(container.querySelector('.brand-menu-ws-row')?.textContent).toContain('Local workspace');
    expect(container.textContent).toContain('Manage workspaces');
  });
  act(() => render(null, container));
});
