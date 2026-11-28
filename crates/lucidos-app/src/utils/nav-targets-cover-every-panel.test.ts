import { describe, it, expect } from 'vitest';
import { NAV_TARGETS, extractNavTargetFromHref } from './linkifyPaths';
import { MENU_ITEMS } from '../store/types';
import { NAVIGATE_TARGETS } from '../../../../packages/lucidos-sdk/src/generated/navigate-targets';

const BARE_PANELS: readonly string[] = [
  'dashboard',
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
