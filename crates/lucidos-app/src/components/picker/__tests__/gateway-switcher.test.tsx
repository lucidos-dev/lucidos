// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { GatewaySwitcher } from '../GatewaySwitcher';
import type { RemoteGatewayView } from '../remoteWorkspaces';

const container = document.createElement('div');

afterEach(() => {
  act(() => render(null, container));
});

const view = (gateway: string, unreachable: string | null = null): RemoteGatewayView => ({
  gateway,
  workspaces: [],
  unreachable,
});

function mount(selected: string | null, disabled = false) {
  const onSelect = vi.fn();
  act(() => {
    render(
      <GatewaySwitcher
        views={[view('lab'), view('studio', 'timed out')]}
        selected={selected}
        disabled={disabled}
        onSelect={onSelect}
      />,
      container,
    );
  });
  const pills = [...container.querySelectorAll<HTMLButtonElement>('.pill-bar-btn')];
  return { onSelect, pills };
}

it('lists this gateway first, then each remote with its reachability', () => {
  const { pills } = mount(null);
  expect(pills.map((pill) => pill.textContent)).toEqual(['This gateway', 'lab', 'studio']);
  expect(pills[0].querySelector('.ws-picker-dot')).toBeNull();
  expect(pills[1].querySelector('.ws-picker-dot-healthy')).not.toBeNull();
  expect(pills[2].querySelector('.ws-picker-dot-unhealthy')).not.toBeNull();
  expect(pills[2].dataset.tooltip).toBe('timed out');
});

it('marks the picked gateway pressed', () => {
  const { pills } = mount('lab');
  expect(pills.map((pill) => pill.getAttribute('aria-pressed'))).toEqual(['false', 'true', 'false']);
  expect(pills[1].classList.contains('active')).toBe(true);
});

it('picks a gateway by name, and this gateway as null', () => {
  const { onSelect, pills } = mount(null);
  act(() => pills[2].click());
  act(() => pills[0].click());
  expect(onSelect.mock.calls).toEqual([['studio'], [null]]);
});

it('locks every pill while an action runs', () => {
  const { pills } = mount(null, true);
  expect(pills.every((pill) => pill.disabled)).toBe(true);
});
