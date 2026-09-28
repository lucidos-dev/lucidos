import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { PluginsView } from '../PluginsView';
import { StoreTab } from '../StoreTab';
import { CatalogFreshness } from '../CatalogFreshness';
import {
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  appSearchOpen,
} from '../../../store/store';

const STORAGE_KEY = 'lucidos-plugins-installed-only';

// Walk a vnode tree WITHOUT invoking function components — their hooks would
// throw outside a real render. Record which function-component types are reached
// (by reference) and every segmented toggle <button>. PluginsView itself has no
// top-level hooks, so calling it as a plain function is safe.
function inspect(
  node: ComponentChildren,
  acc: { types: Set<unknown>; buttons: VNode<Record<string, unknown>>[] },
): void {
  if (node === null || node === undefined || typeof node === 'boolean') return;
  if (typeof node === 'string' || typeof node === 'number') return;
  if (Array.isArray(node)) {
    node.forEach((n) => inspect(n, acc));
    return;
  }
  const v = node as VNode<Record<string, unknown>>;
  if (typeof v.type === 'function') {
    acc.types.add(v.type); // record but do NOT recurse/invoke
    return;
  }
  if (
    v.type === 'button' &&
    typeof v.props?.class === 'string' &&
    v.props.class.includes('segmented-btn')
  ) {
    acc.buttons.push(v);
  }
  inspect(v.props?.children as ComponentChildren, acc);
}

function render() {
  const acc = {
    types: new Set<unknown>(),
    buttons: [] as VNode<Record<string, unknown>>[],
  };
  inspect(PluginsView(), acc);
  return acc;
}

function buttonFor(
  buttons: VNode<Record<string, unknown>>[],
  label: string,
): VNode<Record<string, unknown>> | undefined {
  return buttons.find((b) => b.props?.children === label);
}

function isActive(button: VNode<Record<string, unknown>> | undefined): boolean {
  return typeof button?.props?.class === 'string' && button.props.class.includes('active');
}

describe('PluginsView — All | Installed toggle over one unified list', () => {
  beforeEach(() => {
    appSearchOpen.value = false;
    // Reset to the All (default) state between cases.
    setPluginsInstalledOnly(false);
  });

  it('always renders the unified list (StoreTab) — no separate installed list', () => {
    setPluginsInstalledOnly(false);
    expect(render().types.has(StoreTab)).toBe(true);
    setPluginsInstalledOnly(true);
    expect(render().types.has(StoreTab)).toBe(true);
  });

  it('defaults to All — the "All" segment is active', () => {
    const { buttons } = render();
    expect(buttons.map((b) => b.props?.children)).toEqual(['All', 'Installed']);
    expect(isActive(buttonFor(buttons, 'All'))).toBe(true);
    expect(isActive(buttonFor(buttons, 'Installed'))).toBe(false);
  });

  it('selecting Installed marks the Installed segment active', () => {
    setPluginsInstalledOnly(true);
    const { buttons } = render();
    expect(isActive(buttonFor(buttons, 'Installed'))).toBe(true);
    expect(isActive(buttonFor(buttons, 'All'))).toBe(false);
  });

  it('clicking a segment sets the filter signal', () => {
    expect(pluginsInstalledOnly.value).toBe(false);
    (buttonFor(render().buttons, 'Installed')?.props?.onClick as (() => void) | undefined)?.();
    expect(pluginsInstalledOnly.value).toBe(true);
    (buttonFor(render().buttons, 'All')?.props?.onClick as (() => void) | undefined)?.();
    expect(pluginsInstalledOnly.value).toBe(false);
  });

  it('persists the choice to localStorage so a reload restores the same view', () => {
    setPluginsInstalledOnly(true);
    expect(localStorage.getItem(STORAGE_KEY)).toBe('true');
    setPluginsInstalledOnly(false);
    expect(localStorage.getItem(STORAGE_KEY)).toBe('false');
  });
});

/** The first element vnode carrying `cls`, found without invoking components. */
function findByClass(node: ComponentChildren, cls: string): VNode<Record<string, unknown>> | null {
  if (node === null || node === undefined || typeof node !== 'object') return null;
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = findByClass(n, cls);
      if (hit) return hit;
    }
    return null;
  }
  const v = node as VNode<Record<string, unknown>>;
  if (typeof v.type === 'function') return null;
  if (typeof v.props?.class === 'string' && v.props.class.split(' ').includes(cls)) return v;
  return findByClass(v.props?.children as ComponentChildren, cls);
}

describe('PluginsView: the freshness caption cannot move the list', () => {
  it('holds CatalogFreshness in a line that renders even while it draws nothing', () => {
    // CatalogFreshness renders null until the catalog loads. The wrapper line
    // owns the height, so the rows below do not jump when the age appears.
    const line = findByClass(PluginsView(), 'plugins-freshness-line');
    expect(line).not.toBeNull();
    const acc = { types: new Set<unknown>(), buttons: [] as VNode<Record<string, unknown>>[] };
    inspect(line?.props?.children as ComponentChildren, acc);
    expect(acc.types.has(CatalogFreshness)).toBe(true);
  });
});
