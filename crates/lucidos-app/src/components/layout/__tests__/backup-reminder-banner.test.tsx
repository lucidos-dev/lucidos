/**
 * The app-shell backup reminder, component side: which instance renders, the
 * height it reserves for everything anchored below the header, and the bar's own
 * markup. The visibility rule it consumes (backup off, minus the two-strikes
 * dismissal) is a preference-layer concern and is tested in
 * `store/actions/preferences.test.ts`.
 *
 * Components are invoked as plain functions and the returned vnode tree is
 * walked (the repo idiom, no DOM render library), which is why the markup lives
 * in the hook-free `backupReminderBody` and the gate in `shouldRenderBanner`.
 */
import type { ComponentChildren, VNode } from 'preact';
import { describe, expect, it } from 'vitest';
import {
  backupReminderBody,
  bannerHeightValue,
  shouldRenderBanner,
} from '../BackupReminderBanner';

type AnyVNode = VNode<Record<string, unknown>>;

/** Plain-text content of a vnode subtree (host nodes only). */
function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

/** Host vnodes whose class list includes `cls`. Does not descend into function
 *  components (their hooks would throw outside a real render). */
function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

const NOOP = () => {};

describe('shouldRenderBanner renders exactly one instance per layout', () => {
  // Both instances are mounted: the mobile one inside the fixed header (a flow
  // sibling would sit behind it), the desktop one in the shell's flow. Rendering
  // both would show two bars and, worse, race two ResizeObservers to publish one
  // CSS var.
  it('renders only the desktop instance on a desktop viewport', () => {
    const args = { mobileViewport: false, reminderVisible: true };
    expect(shouldRenderBanner({ layout: 'desktop', ...args })).toBe(true);
    expect(shouldRenderBanner({ layout: 'mobile', ...args })).toBe(false);
  });

  it('renders only the mobile instance on a mobile viewport', () => {
    const args = { mobileViewport: true, reminderVisible: true };
    expect(shouldRenderBanner({ layout: 'mobile', ...args })).toBe(true);
    expect(shouldRenderBanner({ layout: 'desktop', ...args })).toBe(false);
  });

  it('renders neither when the reminder is not due', () => {
    for (const mobileViewport of [true, false]) {
      expect(shouldRenderBanner({ layout: 'desktop', mobileViewport, reminderVisible: false })).toBe(false);
      expect(shouldRenderBanner({ layout: 'mobile', mobileViewport, reminderVisible: false })).toBe(false);
    }
  });
});

describe('bannerHeightValue is the reservation the toast stack and drawer clear', () => {
  it('publishes rem so the reservation survives a UI-scale change', () => {
    expect(bannerHeightValue(32, 16)).toBe('2rem');
    expect(bannerHeightValue(40, 20)).toBe('2rem');
  });

  it('clears rather than reserving zero', () => {
    // A cleared property falls back to the 0px default in base.css, so
    // --app-header-bottom returns to the bare header bottom.
    expect(bannerHeightValue(null, 16)).toBeNull();
    expect(bannerHeightValue(0, 16)).toBeNull();
  });

  it('clears rather than dividing by a bogus root font size', () => {
    expect(bannerHeightValue(32, 0)).toBeNull();
  });
});

describe('backupReminderBody renders the bar', () => {
  const body = () => backupReminderBody({ layout: 'desktop', onSetUp: NOOP, onDismiss: NOOP });

  it('names the risk in plain words rather than only the setting', () => {
    expect(textOf(body())).toContain('Backup is off');
  });

  it('offers the route to switch it on and a way out', () => {
    expect(textOf(body())).toContain('Set up backup');
    expect(findByClass(body(), 'backup-reminder-close')).toHaveLength(1);
  });

  it('carries no left-accent stripe hook', () => {
    // .claude/rules/frontend.md bans a colored vertical border down the left
    // edge of a card / callout / banner outright. Nothing may reintroduce it
    // through a modifier class here.
    const classes = findByClass(body(), 'backup-reminder')[0].props.class as string;
    expect(classes).toBe('backup-reminder');
  });
});
