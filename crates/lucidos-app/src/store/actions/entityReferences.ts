/**
 * EntityReferenceManager — independent SSE consumer that keeps entity references
 * (recents, nav stack, pinned apps, active overlay) in sync when entities change.
 *
 * Wired at the SSE dispatch level in thread-sync.ts, NOT as a side-effect of
 * handleThreadEvent or handleGlobalEvent.
 */
import { panelOverlay, pinnedApps, appsList, triggers } from '../store';
import { loadApps } from './apps';
import { loadTriggers } from './triggers';
import { loadArtifacts } from './artifacts';

export const RECENTS_KEY = 'lucidos-search-recents';
export const NAV_KEY = 'lucidos-nav-history';

/** Process a raw SSE message for entity reference updates. */
export function processSSEForReferences(type: string, data: Record<string, unknown>): void {
  switch (type) {
    // Trigger events
    case 'TriggerCreated':
    case 'TriggerUpdated':
    case 'TriggerEnabled':
    case 'TriggerDisabled':
    case 'TriggerExecuted':
      loadTriggers();
      break;
    case 'TriggerDeleted': {
      const triggerId = data.trigger_id as string;
      if (triggerId) {
        pruneRecents(triggerId, 'triggers');
        pruneNavStack('trigger', triggerId);
        closeOverlayIfStale('trigger', triggerId);
      }
      loadTriggers();
      break;
    }
    // App events
    case 'AppCreated':
      loadApps();
      break;
    case 'AppUpdated': {
      const appId = data.app_id as string;
      const name = data.name as string | undefined;
      if (appId && name) patchRecentsMetadata(appId, 'apps', name);
      loadApps();
      break;
    }
    case 'AppDeleted': {
      const appId = data.app_id as string;
      if (appId) {
        pruneRecents(appId, 'apps');
        pruneNavStack('app', appId);
        prunePinnedApp(appId);
        closeOverlayIfStale('app', appId);
      }
      loadApps();
      break;
    }
    // File events
    case 'ArtifactImported':
      loadArtifacts();
      break;
    // Plugin install/uninstall lands files under apps/, knowhow/, triggers/,
    // scripts/, auth-modules/. Only refresh lists the user has already
    // loaded — eagerly populating caches the user hasn't asked for is pure
    // network + render waste. Knowhow has no list view; scripts/auth-modules
    // don't surface.
    case 'PluginInstalled':
    case 'PluginUninstalled':
      if (appsList.value.status === 'loaded') loadApps();
      if (triggers.value.status === 'loaded') loadTriggers();
      break;
    // Thread title events
    case 'ThreadEvent':
      handleThreadTitleEvent(data);
      break;
  }
}

function handleThreadTitleEvent(data: Record<string, unknown>): void {
  const threadId = data.thread_id as string | undefined;
  const event = data.event as Record<string, unknown> | undefined;
  if (!threadId || !event) return;

  const eventType = event.type as string | undefined;
  if (eventType !== 'ThreadTitleGenerated' && eventType !== 'ThreadTitleRenamed') return;

  const title = event.title as string | undefined;
  if (!title) return;

  patchRecentsMetadata(threadId, 'threads', title);
}

function pruneRecents(id: string, category: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string }> = JSON.parse(raw);
    const filtered = recents.filter(r => !(r.id === id && r.category === category));
    if (filtered.length < recents.length) {
      localStorage.setItem(RECENTS_KEY, JSON.stringify(filtered));
    }
  } catch { /* corrupted localStorage */ }
}

function patchRecentsMetadata(id: string, category: string, name: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string; title: string }> = JSON.parse(raw);
    let changed = false;
    for (const r of recents) {
      if (r.id === id && r.category === category && r.title !== name) {
        r.title = name;
        changed = true;
      }
    }
    if (changed) localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
  } catch { /* corrupted localStorage */ }
}

function pruneNavStack(entity: string, id: string): void {
  try {
    const raw = localStorage.getItem(NAV_KEY);
    if (!raw) return;
    const { stack, cursor }: { stack: Array<Record<string, unknown>>; cursor: number } = JSON.parse(raw);
    if (!Array.isArray(stack)) return;

    let removed = 0;
    const filtered = stack.filter((entry, i) => {
      if (isNavEntryStale(entry, entity, id)) {
        if (i <= cursor) removed++;
        return false;
      }
      return true;
    });

    if (filtered.length < stack.length) {
      const newCursor = Math.max(0, Math.min(cursor - removed, filtered.length - 1));
      localStorage.setItem(NAV_KEY, JSON.stringify({ stack: filtered, cursor: newCursor }));
    }
  } catch { /* corrupted localStorage */ }
}

function isNavEntryStale(entry: Record<string, unknown>, entity: string, id: string): boolean {
  const overlay = entry.overlay as Record<string, unknown> | null;
  if (!overlay) return false;

  switch (entity) {
    case 'app':
      if (overlay.type === 'app-ui') {
        const app = overlay.app as Record<string, unknown> | undefined;
        return app?.id === id;
      }
      if (overlay.type === 'form') {
        const form = overlay.form as Record<string, unknown> | undefined;
        return form?.type === 'app-edit' && form?.appId === id;
      }
      return false;
    case 'trigger':
      if (overlay.type === 'form') {
        const form = overlay.form as Record<string, unknown> | undefined;
        return form?.type === 'trigger' && form?.taskId === id;
      }
      return false;
    default:
      return false;
  }
}

function prunePinnedApp(appId: string): void {
  const current = pinnedApps.value;
  if (!current.some(e => e.app_id === appId)) return;
  const filtered = current.filter(e => e.app_id !== appId);
  pinnedApps.value = filtered;
  localStorage.setItem('pinned_apps', JSON.stringify(filtered));
}

function closeOverlayIfStale(entity: string, id: string): void {
  const overlay = panelOverlay.value;
  if (!overlay) return;

  switch (entity) {
    case 'app':
      if (overlay.type === 'app-ui' && overlay.app.id === id) {
        panelOverlay.value = null;
        localStorage.removeItem('app-window-open');
      }
      break;
    case 'trigger':
      if (overlay.type === 'form' && overlay.form.type === 'trigger' && 'taskId' in overlay.form && overlay.form.taskId === id) {
        panelOverlay.value = null;
      }
      break;
  }
}
