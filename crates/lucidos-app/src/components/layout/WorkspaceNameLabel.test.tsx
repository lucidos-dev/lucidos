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
it('identifies the remote gateway in the header inside dev@cloud, without a redundant "remote"', () => {
  workspaceName.value = 'dev';
  const container = document.createElement('div');
  act(() => render(<WorkspaceNameLabel />, container));
  expect(container.querySelector('.remote-workspace-gateway')?.textContent).toBe('@cloud');
  expect(container.textContent).toBe('dev@cloud');
});

/** Renders the label inside the brand box with stubbed geometry: the mark is
 *  30px, the name 100px, the `@cloud` suffix 60px (margin included). */
function renderInBox(boxWidth: number): HTMLElement {
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  const brand = document.createElement('span');
  brand.className = 'pane-header-brand';
  const box = document.createElement('span');
  box.className = 'pane-header-brand-center';
  brand.appendChild(box);
  Object.defineProperty(box, 'clientWidth', { value: boxWidth });
  const mark = document.createElement('span');
  const host = document.createElement('span');
  box.append(mark, host);
  const scrollWidth = vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockImplementation(function (this: HTMLElement) {
    if (this === mark) return 30;
    if (!this.classList.contains('workspace-name-label')) return 0;
    const suffix = this.querySelector('.remote-workspace-gateway');
    return suffix?.classList.contains('is-collapsed') ? 100 : 160;
  });
  const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return { width: this.classList.contains('remote-workspace-gateway') ? 60 : 0 } as DOMRect;
  });
  workspaceName.value = 'dev';
  act(() => render(<WorkspaceNameLabel />, host));
  scrollWidth.mockRestore();
  rect.mockRestore();
  vi.unstubAllGlobals();
  return host;
}

const labelOf = (host: HTMLElement) => host.querySelector('.workspace-name-label')!;
const suffixOf = (host: HTMLElement) => host.querySelector('.remote-workspace-gateway')!;

it('shows the name with its gateway when both fit', () => {
  const host = renderInBox(200);
  expect(labelOf(host).classList.contains('is-hidden')).toBe(false);
  expect(suffixOf(host).classList.contains('is-collapsed')).toBe(false);
});

it('drops the gateway before the name when only the name fits', () => {
  const host = renderInBox(150);
  expect(labelOf(host).classList.contains('is-hidden')).toBe(false);
  expect(suffixOf(host).classList.contains('is-collapsed')).toBe(true);
});

it('hides the label when not even the bare name fits', () => {
  const host = renderInBox(100);
  expect(labelOf(host).classList.contains('is-hidden')).toBe(true);
});
