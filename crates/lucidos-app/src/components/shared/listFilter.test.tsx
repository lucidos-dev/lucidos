// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { filterListItems, listFilterVisible } from './useListFilter';
import { Dropdown } from './Dropdown';
import { ModelSelectionPicker } from './ModelSelectionPicker';
import { useModelSelection } from '../../hooks/useModelSelection';
import { WidgetPickerItems } from '../widgets/WidgetPickerItems';
import { appsList } from '../../store/store';
import { reusableWidgets } from '../../store/widgets';
import { topOverlay } from '../../store/overlayStack';
import type { App } from '../../store/types';

const device = vi.hoisted(() => ({ coarse: false, narrow: false }));
vi.mock('../../utils/viewport', async (original) => ({
  ...(await original<typeof import('../../utils/viewport')>()),
  hasCoarsePointer: () => device.coarse,
  isMobile: () => device.narrow,
}));

const options = [
  { value: 'alpha', label: 'Alpha', description: 'Fast replies' },
  { value: 'beta', label: 'Beta', description: 'Deep reasoning' },
];

describe('the shared dropdown filter', () => {
  it('matches names and descriptions without case sensitivity', () => {
    expect(filterListItems(options, ' ALP ')).toEqual([options[0]]);
    expect(filterListItems(options, 'DEEP')).toEqual([options[1]]);
    expect(filterListItems(options, 'beta reasoning')).toEqual([options[1]]);
    expect(filterListItems(options, 'missing')).toEqual([]);
    expect(filterListItems(options, '  ')).toBe(options);
  });

  it.each([
    ['', false, false, false], ['x', false, false, true],
    ['', true, false, true], ['', false, true, true],
  ])('query %j, coarse %s, narrow %s: visible %s', (query, coarse, narrow, visible) => {
    expect(listFilterVisible({ query, coarse, narrow })).toBe(visible);
  });
});

let host: HTMLDivElement;
beforeEach(() => {
  device.coarse = false;
  device.narrow = false;
  vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {});
  host = document.createElement('div');
  document.body.appendChild(host);
});
afterEach(() => {
  act(() => render(null, host));
  host.remove();
  appsList.value = { status: 'not-loaded' };
  reusableWidgets.value = { status: 'not-loaded' };
  vi.restoreAllMocks();
});

function ModelPicker({ onPick }: { onPick: (value: string) => void }) {
  const selection = useModelSelection({
    models: options.map((o) => ({ ...o, reasoningEfforts: [] })),
    vocabulary: [], model: 'alpha', effort: null, onChange: () => {},
  });
  return <ModelSelectionPicker label="Models" selection={selection} onPick={onPick} />;
}

function key(target: Element, value: string) {
  act(() => { target.dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true })); });
}

function mount(kind: string, onPick = vi.fn()) {
  act(() => {
    if (kind === 'dropdown') {
      render(<Dropdown options={options} value="alpha" onChange={onPick} />, host);
      host.querySelector<HTMLButtonElement>('button')!.click();
    } else if (kind === 'model') render(<ModelPicker onPick={onPick} />, host);
    else {
      reusableWidgets.value = { status: 'loaded', data: [] };
      appsList.value = { status: 'loaded', data: options.map((o) => ({
        id: o.value, name: o.label, description: o.description, kind: 'app',
      } as App)) };
      render(<WidgetPickerItems target={{ dashboard: 'desktop' }} run={(fn) => () => { onPick(); fn(); }} />, host);
    }
  });
  const root = kind === 'dropdown' ? document.body.querySelector('.dropdown-menu')! : host;
  const keyboard = kind === 'dropdown' ? host.querySelector('button')!
    : root.querySelector('[tabindex="0"]')!;
  return { root, keyboard };
}

for (const kind of ['dropdown', 'model', 'dashboard']) {
  describe(`${kind} uses the shared filter`, () => {
    it('reveals the seed, matches descriptions, and hides after deleting to empty', async () => {
      const { root, keyboard } = mount(kind);
      expect(root.querySelector('input')).toBeNull();
      key(keyboard, 'd');
      const input = root.querySelector('input')!;
      expect(input.value).toBe('d');
      await vi.waitFor(() => expect(document.activeElement).toBe(input));
      expect(root.textContent).toContain('Beta');
      expect(root.textContent).not.toContain('Alpha');
      act(() => {
        input.value = '';
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      expect(root.querySelector('input')).toBeNull();
      expect(root.textContent).toContain('Alpha');
      await vi.waitFor(() => expect(document.activeElement).not.toBe(document.body));
    });

    it.each(['coarse', 'narrow'] as const)('always shows the field on %s layouts', (layout) => {
      device[layout] = true;
      expect(mount(kind).root.querySelector('input')).not.toBeNull();
    });

    it('shows no matches and Escape clears before leaving the list', () => {
      const { root, keyboard } = mount(kind);
      key(keyboard, 'z');
      expect(root.textContent).toContain('No matches');
      act(() => topOverlay()?.dismiss());
      expect(root.textContent).toContain('Alpha');
      expect(root.querySelector('input')).toBeNull();
      if (kind === 'dropdown') {
        expect(host.querySelector('button')?.getAttribute('aria-expanded')).toBe('true');
        act(() => topOverlay()?.dismiss());
        expect(host.querySelector('button')?.getAttribute('aria-expanded')).toBe('false');
      }
    });

    it('selects only a filtered row with arrows and Enter', () => {
      const onPick = vi.fn();
      const { root, keyboard } = mount(kind, onPick);
      key(keyboard, 'b');
      const input = root.querySelector('input')!;
      key(input, 'ArrowDown');
      key(input, 'ArrowUp');
      key(input, 'Enter');
      expect(onPick).toHaveBeenCalledOnce();
      if (kind === 'dropdown') expect(onPick).toHaveBeenCalledWith('beta');
      if (kind === 'model') expect(onPick.mock.calls[0][0]).toContain('beta');
    });
  });
}

it('seeds a closed standard dropdown and selects the first filtered result', () => {
  const onPick = vi.fn();
  act(() => render(<Dropdown options={options} value="alpha" onChange={onPick} />, host));
  const trigger = host.querySelector('button')!;
  key(trigger, 'b');
  key(trigger, 'Enter');
  expect(onPick).toHaveBeenCalledWith('beta');
});

it('lets Backspace clear a dashboard query after navigating to a result', async () => {
  const { root, keyboard } = mount('dashboard');
  key(keyboard, 'b');
  const input = root.querySelector('input')!;
  await vi.waitFor(() => expect(document.activeElement).toBe(input));
  key(input, 'ArrowDown');
  expect(document.activeElement?.getAttribute('role')).toBe('menuitem');
  key(document.activeElement!, 'Backspace');
  expect(root.querySelector('input')).toBeNull();
  expect(root.textContent).toContain('Alpha');
});
