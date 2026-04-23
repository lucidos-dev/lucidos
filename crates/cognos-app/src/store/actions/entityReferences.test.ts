import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, pinnedApps } from '../store';
import type { App } from '../types';

// Mock loader functions to prevent API calls
vi.mock('./apps', () => ({ loadApps: vi.fn() }));
vi.mock('./triggers', () => ({ loadTriggers: vi.fn() }));
vi.mock('./artifacts', () => ({ loadArtifacts: vi.fn() }));

import { processSSEForReferences } from './entityReferences';
import { loadApps } from './apps';
import { loadTriggers } from './triggers';
import { loadArtifacts } from './artifacts';

const RECENTS_KEY = 'cognos-search-recents';
const NAV_KEY = 'cognos-nav-history';

function setRecents(recents: Array<{ id: string; category: string; title: string }>): void {
  localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
}

function getRecents(): Array<{ id: string; category: string; title: string }> {
  const raw = localStorage.getItem(RECENTS_KEY);
  return raw ? JSON.parse(raw) : [];
}

function setNavStack(stack: Array<Record<string, unknown>>, cursor: number): void {
  localStorage.setItem(NAV_KEY, JSON.stringify({ stack, cursor }));
}

function getNavStack(): { stack: Array<Record<string, unknown>>; cursor: number } | null {
  const raw = localStorage.getItem(NAV_KEY);
  return raw ? JSON.parse(raw) : null;
}

const testApp: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '', knowhow: [] };

describe('processSSEForReferences', () => {
  beforeEach(() => {
    localStorage.clear();
    panelOverlay.value = null;
    pinnedApps.value = [];
    vi.clearAllMocks();
  });

  // ── Deleted app ──────────────────────────────────────────────────────────

  describe('AppDeleted', () => {
    it('prunes app from recents localStorage', () => {
      setRecents([
        { id: 'habit-tracker', category: 'apps', title: 'Habit Tracker' },
        { id: 'some-thread', category: 'threads', title: 'My Thread' },
      ]);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const recents = getRecents();
      expect(recents).toHaveLength(1);
      expect(recents[0].id).toBe('some-thread');
    });

    it('prunes app-ui entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
        { overlay: null },
        { overlay: { type: 'app-ui', app: { id: 'other-app' } } },
      ], 2);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(2);
      expect(nav.stack.every((e: Record<string, unknown>) => {
        const o = e.overlay as Record<string, unknown> | null;
        return !o || o.type !== 'app-ui' || (o.app as Record<string, unknown>).id !== 'habit-tracker';
      })).toBe(true);
    });

    it('prunes app-edit form entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'form', form: { type: 'app-edit', appId: 'habit-tracker' } } },
        { overlay: null },
      ], 1);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect((nav.stack[0] as any).overlay).toBeNull();
    });

    it('prunes from pinned apps signal', () => {
      pinnedApps.value = [{ app_id: 'habit-tracker' }, { app_id: 'other-app' }];
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(pinnedApps.value).toEqual([{ app_id: 'other-app' }]);
    });

    it('closes app-ui overlay if matching app is open', () => {
      panelOverlay.value = { type: 'app-ui', app: testApp };
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(panelOverlay.value).toBeNull();
    });

    it('does not close overlay for different app', () => {
      panelOverlay.value = { type: 'app-ui', app: { ...testApp, id: 'other-app' } };
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(panelOverlay.value).not.toBeNull();
      expect(panelOverlay.value!.type).toBe('app-ui');
    });

    it('calls loadApps', () => {
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(loadApps).toHaveBeenCalled();
    });
  });

  // ── Deleted trigger ──────────────────────────────────────────────────────

  describe('TriggerDeleted', () => {
    it('prunes trigger from recents', () => {
      setRecents([
        { id: 'daily-check', category: 'triggers', title: 'Daily Check' },
        { id: 'other', category: 'apps', title: 'Other' },
      ]);
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      const recents = getRecents();
      expect(recents).toHaveLength(1);
      expect(recents[0].id).toBe('other');
    });

    it('prunes trigger form entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'form', form: { type: 'trigger', taskId: 'daily-check' } } },
        { overlay: null },
      ], 0);
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect((nav.stack[0] as any).overlay).toBeNull();
    });

    it('closes trigger form overlay if matching trigger is open', () => {
      panelOverlay.value = { type: 'form', form: { type: 'trigger', taskId: 'daily-check' } };
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      expect(panelOverlay.value).toBeNull();
    });

    it('calls loadTriggers', () => {
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });
  });

  // ── Trigger lifecycle events ────────────────────────────────────────────

  describe('trigger lifecycle events', () => {
    it('TriggerCreated calls loadTriggers', () => {
      processSSEForReferences('TriggerCreated', { trigger_id: 'new-trigger' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerUpdated calls loadTriggers', () => {
      processSSEForReferences('TriggerUpdated', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerEnabled calls loadTriggers', () => {
      processSSEForReferences('TriggerEnabled', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerDisabled calls loadTriggers', () => {
      processSSEForReferences('TriggerDisabled', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerExecuted calls loadTriggers', () => {
      processSSEForReferences('TriggerExecuted', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });
  });

  // ── AppCreated ──────────────────────────────────────────────────────────

  describe('AppCreated', () => {
    it('calls loadApps', () => {
      processSSEForReferences('AppCreated', { app_id: 'new-app', name: 'New App' });
      expect(loadApps).toHaveBeenCalled();
    });
  });

  // ── ArtifactImported ────────────────────────────────────────────────────

  describe('ArtifactImported', () => {
    it('calls loadArtifacts', () => {
      processSSEForReferences('ArtifactImported', { artifact_path: 'notes.md', source_type: 'local_file', source_detail: '/tmp/notes.md', commit_hash: 'abc' });
      expect(loadArtifacts).toHaveBeenCalled();
    });
  });

  // ── Updated app with name ────────────────────────────────────────────────

  describe('AppUpdated', () => {
    it('patches recents title in localStorage', () => {
      setRecents([
        { id: 'habit-tracker', category: 'apps', title: 'Old Name' },
        { id: 'other', category: 'threads', title: 'Other' },
      ]);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'New Name' });
      const recents = getRecents();
      expect(recents[0].title).toBe('New Name');
      expect(recents[1].title).toBe('Other');
    });

    it('does not patch if name matches already', () => {
      setRecents([{ id: 'habit-tracker', category: 'apps', title: 'Same Name' }]);
      const before = localStorage.getItem(RECENTS_KEY);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'Same Name' });
      // localStorage.setItem should not be called when title already matches
      expect(localStorage.getItem(RECENTS_KEY)).toBe(before);
    });

    it('does not patch recents when name is absent', () => {
      setRecents([{ id: 'habit-tracker', category: 'apps', title: 'Old Name' }]);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker' });
      const recents = getRecents();
      expect(recents[0].title).toBe('Old Name');
    });

    it('calls loadApps', () => {
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'New Name' });
      expect(loadApps).toHaveBeenCalled();
    });
  });

  // ── ThreadTitleGenerated / ThreadTitleRenamed ─────────────────────────────

  describe('ThreadTitleGenerated', () => {
    it('patches thread recents title', () => {
      setRecents([
        { id: 'thread-abc', category: 'threads', title: 'Old Title' },
        { id: 'other', category: 'apps', title: 'Other' },
      ]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'ThreadTitleGenerated', title: 'New Title' },
      });
      const recents = getRecents();
      expect(recents[0].title).toBe('New Title');
      expect(recents[1].title).toBe('Other');
    });

    it('patches thread recents title for ThreadTitleRenamed', () => {
      setRecents([{ id: 'thread-xyz', category: 'threads', title: 'Old' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-xyz',
        event: { type: 'ThreadTitleRenamed', title: 'Renamed' },
      });
      expect(getRecents()[0].title).toBe('Renamed');
    });

    it('ignores ThreadEvent with non-title event type', () => {
      setRecents([{ id: 'thread-abc', category: 'threads', title: 'Original' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'MessageReceived', text: 'hello' },
      });
      expect(getRecents()[0].title).toBe('Original');
    });

    it('ignores ThreadEvent without title in event', () => {
      setRecents([{ id: 'thread-abc', category: 'threads', title: 'Original' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'ThreadTitleGenerated' },
      });
      expect(getRecents()[0].title).toBe('Original');
    });
  });

  // ── Graceful no-ops ──────────────────────────────────────────────────────

  describe('graceful no-ops', () => {
    it('does not crash for unknown SSE type', () => {
      expect(() => processSSEForReferences('UnknownEvent', { foo: 'bar' })).not.toThrow();
    });

    it('does not crash when ThreadEvent has no event field', () => {
      expect(() => processSSEForReferences('ThreadEvent', { thread_id: 'abc' })).not.toThrow();
    });

    it('does not crash when ThreadEvent has no thread_id', () => {
      expect(() => processSSEForReferences('ThreadEvent', { event: { type: 'ThreadTitleGenerated', title: 'x' } })).not.toThrow();
    });

    it('does not crash for AppDeleted with missing app_id', () => {
      expect(() => processSSEForReferences('AppDeleted', {})).not.toThrow();
    });

    it('does not crash for TriggerDeleted with missing trigger_id', () => {
      expect(() => processSSEForReferences('TriggerDeleted', {})).not.toThrow();
    });
  });

  // ── Corrupted localStorage ───────────────────────────────────────────────

  describe('corrupted localStorage', () => {
    it('does not crash when recents is invalid JSON', () => {
      localStorage.setItem(RECENTS_KEY, '{{not json');
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });

    it('does not crash when nav stack is invalid JSON', () => {
      localStorage.setItem(NAV_KEY, '{{not json');
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });

    it('does not crash when nav stack has no array', () => {
      localStorage.setItem(NAV_KEY, JSON.stringify({ stack: 'not-array', cursor: 0 }));
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });
  });

  // ── Nav stack cursor adjustment ──────────────────────────────────────────

  describe('nav stack cursor adjustment', () => {
    it('adjusts cursor when pruned entry is before cursor', () => {
      setNavStack([
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
        { overlay: null },
        { overlay: { type: 'file-preview', path: 'notes.md' } },
      ], 2);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(2);
      expect(nav.cursor).toBe(1);
    });

    it('does not adjust cursor when pruned entry is after cursor', () => {
      setNavStack([
        { overlay: null },
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
      ], 0);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect(nav.cursor).toBe(0);
    });
  });
});
