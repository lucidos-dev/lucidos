/**
 * `NAV_TARGETS` decides whether a bare markdown link is a panel or a dead link.
 *
 * It is a hand-written set, and the two lists it has to agree with are not:
 * `MENU_ITEMS` is the drawer, and the generated `NAVIGATE_TARGETS` is what the
 * engine's `navigate_ui` tool accepts. Both moved without it. `plugins` was a
 * menu item and a generated target for months, while `[Plugins](plugins)`
 * toasted "points nowhere in this workspace". The set knew only the
 * `app-store` alias ADR 0019 retired.
 *
 * So the partition below is the decision a reader has to make once per target,
 * recorded where a new one cannot slip past it: a bare name either opens a
 * panel, or it needs something the href does not carry.
 */
import { describe, it, expect } from 'vitest';
import { NAV_TARGETS, extractNavTargetFromHref } from './linkifyPaths';
import { MENU_ITEMS } from '../store/types';
import { NAVIGATE_TARGETS } from '../../../../packages/lucidos-sdk/src/generated/navigate-targets';

/** Targets a bare `[Label](name)` link opens on its own. */
const BARE_PANELS: readonly string[] = [
  'files',
  'apps',
  'plugins',
  'app-store',
  'triggers',
  'thread-queue',
  'changes',
  'notifications',
  'settings',
];

/** Targets a bare name cannot express, each mapped to why.
 *  The first five take an id or a path, and their own extractors claim those
 *  hrefs (`app:`, `trigger:`, `artifacts/…`). The last three are actions, not
 *  destinations. A link that ran one on a click would be a side effect wearing
 *  a panel's clothes. */
const NEEDS_MORE_THAN_A_NAME: Readonly<Record<string, string>> = {
  app: 'takes an app_id',
  file: 'takes a file_path',
  trigger: 'takes an id',
  thread: 'takes an id',
  url: 'takes a url',
  'new-app': 'opens a form',
  'new-trigger': 'opens a form',
  'new-chat': 'starts a thread',
};

describe('NAV_TARGETS covers every panel a link can name', () => {
  it('classifies every generated navigate target exactly once', () => {
    const classified = [...BARE_PANELS, ...Object.keys(NEEDS_MORE_THAN_A_NAME)].sort();
    expect(classified).toEqual([...NAVIGATE_TARGETS].sort());
  });

  it('links every menu item, so a new drawer row is clickable from chat', () => {
    for (const item of MENU_ITEMS) expect(BARE_PANELS).toContain(item);
  });

  it('holds exactly the bare panels, and nothing it invented', () => {
    expect([...NAV_TARGETS].sort()).toEqual([...BARE_PANELS].sort());
  });

  it.each(BARE_PANELS)('resolves %s to itself', (name) => {
    expect(extractNavTargetFromHref(name)).toBe(name);
  });

  it.each(Object.entries(NEEDS_MORE_THAN_A_NAME))('leaves %s alone: it %s', (name) => {
    expect(extractNavTargetFromHref(name)).toBeNull();
  });
});
