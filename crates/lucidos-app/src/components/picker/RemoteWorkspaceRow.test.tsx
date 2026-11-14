// @vitest-environment jsdom
/**
 * A picker row for a remote workspace wears the local row's shape. It offers
 * nothing its own gateway owns, and stays shut while that gateway is silent.
 * A middle press means "open this beside", where there is somewhere to.
 */
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { RemoteGatewayView } from './remoteWorkspaces';

const platform = vi.hoisted(() => ({ isStandalone: false }));
vi.mock('../../utils/platform', async () => ({
  ...await vi.importActual('../../utils/platform'),
  isTauri: () => false,
  isStandalone: () => platform.isStandalone,
}));

const { RemoteWorkspaceRowBody, UnreachableGatewayNotes } = await import('./RemoteWorkspaceRow');

const container = document.createElement('div');

afterEach(() => {
  act(() => render(null, container));
  platform.isStandalone = false;
});

const cloud = (unreachable: string | null = null): RemoteGatewayView => ({
  gateway: 'cloud',
  host: 'cloud.example.net',
  workspaces: [{ id: 'dev', name: 'dev', health: 'healthy' }],
  unreachable,
});

function mount(view: RemoteGatewayView): ReturnType<typeof vi.fn> {
  const onOpen = vi.fn();
  act(() => {
    render(
      <RemoteWorkspaceRowBody view={view} workspace={view.workspaces[0]} onOpen={onOpen} actions={<button>⋯</button>} />,
      container,
    );
  });
  return onOpen;
}

it('draws the name, a quiet @gateway tag and the host line', () => {
  mount(cloud());
  expect(container.querySelector('.ws-picker-name')?.textContent).toBe('dev @cloud');
  expect(container.querySelector('.ws-picker-gateway-tag')?.textContent).toBe('@cloud');
  expect(container.querySelector('.ws-picker-address')?.textContent).toBe('On cloud.example.net');
});

it('offers only the actions it is handed, never Start, Stop, rename or delete', () => {
  mount(cloud());
  const buttons = [...container.querySelectorAll('button')].map((b) => b.textContent);
  expect(buttons).toEqual(['⋯']);
});

it('stays shut while its gateway is silent', () => {
  const onOpen = mount(cloud('timed out'));
  const row = container.querySelector<HTMLElement>('.ws-picker-open')!;
  expect(row.getAttribute('aria-disabled')).toBe('true');
  act(() => { row.click(); });
  expect(onOpen).not.toHaveBeenCalled();
});

it('names each silent gateway and why', () => {
  act(() => { render(<UnreachableGatewayNotes views={[cloud('timed out'), { ...cloud(), gateway: 'lab' }]} />, container); });
  const notes = [...container.querySelectorAll('.ws-picker-row-note')].map((n) => n.textContent);
  expect(notes).toEqual(['timed out']);
});

/** Press the wheel on a healthy `dev@cloud` row, and return what opened. */
function middleClick(): ReturnType<typeof vi.fn> {
  const onOpen = mount(cloud());
  const row = container.querySelector<HTMLElement>('.ws-picker-open[role="button"]')!;
  act(() => { row.dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true, cancelable: true })); });
  return onOpen;
}

it('opens a separate view on a middle click in a browser', () => {
  expect(middleClick()).toHaveBeenCalledWith('dev@cloud', 'separate');
});

it('ignores a middle click in an installed PWA, which has nowhere to open a separate view', () => {
  platform.isStandalone = true;
  expect(middleClick()).not.toHaveBeenCalled();
});
