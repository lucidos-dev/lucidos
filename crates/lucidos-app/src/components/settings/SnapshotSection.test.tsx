// @vitest-environment jsdom
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { SnapshotSection } from './SnapshotSection';
import { json, mutatingFetch, requestGenerationGuard } from '../../api/client/_core';
import { showConfirm, showToast } from '../../store/store';

const context = vi.hoisted(() => ({ workspace: 'demo' as string | null }));
vi.mock('../../utils/basePath', async original => ({ ...await original<typeof import('../../utils/basePath')>(), API: '/api/v1', get WORKSPACE_ID() { return context.workspace; } }));
vi.mock('../../api/client/_core', async original => ({
  ...await original<typeof import('../../api/client/_core')>(),
  json: vi.fn(), mutatingFetch: vi.fn(), requestGenerationGuard: vi.fn(() => () => {}),
}));
vi.mock('../../store/store', async original => ({
  ...await original<typeof import('../../store/store')>(), showToast: vi.fn(), showConfirm: vi.fn(),
}));
vi.mock('../../hooks/usePanelRefresh', () => ({ usePanelRefresh: vi.fn() }));
const entry = {
  handle: 'snap_example', kind: 'manual', label: 'Before changes', keep: false,
  workspace_id: 'demo', workspace_name: 'Demo workspace',
  created_at: '2026-01-01T12:00:00Z', components: [{ bytes: 1024 }],
};
let host: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  context.workspace = 'demo';
  vi.mocked(json).mockResolvedValue([entry]);
  vi.mocked(requestGenerationGuard).mockReturnValue(Object.assign(() => {}, { headers: {} }));
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { render(null, host); host.remove(); document.querySelector('meta[name="lucidos-workspace-id"]')?.remove(); });
async function open() {
  await act(async () => { render(<SnapshotSection />, host); });
  await act(async () => {});
}
async function restore() {
  await act(async () => { [...host.querySelectorAll('button')].find(b => b.textContent === 'Restore')!.click(); });
}

it('names the target and external side effects before a canceled restore sends nothing', async () => {
  vi.mocked(showConfirm).mockResolvedValue(false);
  await open(); await restore();
  expect(showConfirm).toHaveBeenCalledWith(expect.stringContaining('Demo workspace'), 'Restore workspace', expect.any(Object));
  expect(vi.mocked(showConfirm).mock.calls[0][0]).toContain('external side effects');
  expect(mutatingFetch).not.toHaveBeenCalled();
});

it('refuses a restore when its generation changes while confirmation is open', async () => {
  vi.mocked(showConfirm).mockResolvedValue(true);
  vi.mocked(requestGenerationGuard).mockReturnValue(Object.assign(() => { throw new Error('Generation changed'); }, { headers: {} }));
  await open(); await restore();
  expect(mutatingFetch).not.toHaveBeenCalled();
});

it('shows a failed snapshot read without presenting an empty list', async () => {
  vi.mocked(json).mockRejectedValue(new Error('Snapshot service unavailable'));
  await open();
  expect(host.textContent).toContain('Snapshot service unavailable');
  expect(host.textContent).not.toContain('No workspace snapshots yet');
});

it('uses the gateway-stamped identity on a direct engine URL', async () => {
  context.workspace = null;
  const meta = document.createElement('meta');
  meta.name = 'lucidos-workspace-id'; meta.content = 'demo'; document.head.append(meta);
  vi.mocked(showConfirm).mockResolvedValue(false);
  await open(); await restore();
  expect(showConfirm).toHaveBeenCalledWith(expect.stringContaining('Demo workspace'), 'Restore workspace', expect.any(Object));
  expect(mutatingFetch).not.toHaveBeenCalled();
});

it('refuses a direct engine identity that differs from the snapshot', async () => {
  context.workspace = null;
  const meta = document.createElement('meta');
  meta.name = 'lucidos-workspace-id'; meta.content = 'another-workspace'; document.head.append(meta);
  await open(); await restore();
  expect(showConfirm).not.toHaveBeenCalled();
  expect(mutatingFetch).not.toHaveBeenCalled();
  expect(showToast).toHaveBeenCalledWith(expect.stringContaining('its workspace'), 'error', expect.any(Object));
});

it('says a gateway-less engine cannot restore, rather than naming another workspace', async () => {
  context.workspace = null;
  vi.mocked(json).mockResolvedValue([{ ...entry, workspace_id: 'database:lucidos_demo' }]);
  await open(); await restore();
  expect(showConfirm).not.toHaveBeenCalled();
  expect(mutatingFetch).not.toHaveBeenCalled();
  expect(showToast).toHaveBeenCalledWith(expect.stringContaining('gateway'), 'error', expect.any(Object));
  expect(showToast).not.toHaveBeenCalledWith(expect.stringContaining('its workspace'), 'error', expect.any(Object));
});
