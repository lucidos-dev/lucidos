import { readWindowItem, removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
import { signal, computed, effect } from '@preact/signals';
import {
  activeMenuItem,
  settingsSubview,
  panelOverlay,
  webviewInitialUrl,
  wipPreviewThreadId,
  migrateSettingsSubview,
} from '../store';
import type { SettingsSubview, InlineForm, PanelOverlay } from '../store';
import type { MenuItem } from '../types';
import { MENU_ITEMS } from '../types';
import { inAppBrowserRenders, normalizeUrl } from './artifacts';
import { inAppBrowserAvailable } from './preferences';
import { revealContentPane } from './pane';

export const NAV_KEY = 'lucidos-nav-history';

export interface NavEntry {
  menuItem: MenuItem;
  settingsSubview: SettingsSubview;
  overlay: PanelOverlay;
  wipPreviewThreadId: string | null;
}

function inlineFormsEqual(a: InlineForm | null, b: InlineForm | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.type !== b.type) return false;
  switch (a.type) {
    case 'credential': {
      const bb = b as typeof a;
      return a.editing === bb.editing && a.request?.service === bb.request?.service;
    }
    case 'app-edit': return a.appId === (b as typeof a).appId;
    case 'new-app': return true;
    case 'trigger': return a.triggerId === (b as typeof a).triggerId;
    case 'email-confirm': {
      const bb = b as typeof a;
      const ar = a.request;
      const br = bb.request;
      return (
        a.sentAt === bb.sentAt &&
        ar.subject === br.subject &&
        ar.to.length === br.to.length &&
        ar.to.every((addr, i) => addr === br.to[i])
      );
    }
    case 'plugin-install': {
      const bb = b as typeof a;
      return a.request.install_id === bb.request.install_id
        && a.installed?.at === bb.installed?.at;
    }
    case 'plugin-uninstall': {
      const bb = b as typeof a;
      return a.request.uninstall_id === bb.request.uninstall_id
        && a.removed?.at === bb.removed?.at;
    }
  }
}

function overlaysEqual(a: PanelOverlay, b: PanelOverlay): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.type !== b.type) return false;
  switch (a.type) {
    case 'form': return inlineFormsEqual(a.form, (b as typeof a).form);
    case 'app-ui': return a.app.id === (b as typeof a).app.id;
    case 'file-preview': return a.path === (b as typeof a).path;
    case 'url-preview': return a.url === (b as typeof a).url;
    case 'notification-detail': return a.notification.id === (b as typeof a).notification.id;
    case 'plugin-detail': {
      const bb = b as typeof a;
      return a.marketplaceId === bb.marketplaceId && a.pluginId === bb.pluginId;
    }
  }
}

export function statesEqual(a: NavEntry, b: NavEntry): boolean {
  return (
    a.menuItem === b.menuItem &&
    a.settingsSubview === b.settingsSubview &&
    overlaysEqual(a.overlay, b.overlay) &&
    a.wipPreviewThreadId === b.wipPreviewThreadId
  );
}

const MAX_NAV_STACK = 50;

export function pushEntry(
  stack: NavEntry[],
  cursor: number,
  entry: NavEntry,
): { stack: NavEntry[]; cursor: number } | null {
  if (cursor < stack.length) {
    if (
      entry.overlay
      && overlaysEqual(entry.overlay, stack[cursor].overlay)
      && entry.wipPreviewThreadId === stack[cursor].wipPreviewThreadId
    ) return null;
    if (statesEqual(entry, stack[cursor])) return null;
  }
  let newStack = [...stack.slice(0, cursor + 1), entry];
  let newCursor = newStack.length - 1;
  if (newStack.length > MAX_NAV_STACK) {
    const overflow = newStack.length - MAX_NAV_STACK;
    newStack = newStack.slice(overflow);
    newCursor -= overflow;
  }
  return { stack: newStack, cursor: newCursor };
}

export function foldEntry(
  stack: NavEntry[],
  cursor: number,
  entry: NavEntry,
): { stack: NavEntry[]; cursor: number } | null {
  if (cursor < 0 || cursor >= stack.length) return null;
  if (statesEqual(entry, stack[cursor])) return null;
  const newStack = [...stack];
  newStack[cursor] = entry;
  let newCursor = cursor;
  if (newCursor + 1 < newStack.length && statesEqual(newStack[newCursor + 1], entry)) {
    newStack.splice(newCursor + 1, 1);
  }
  if (newCursor > 0 && statesEqual(newStack[newCursor - 1], entry)) {
    newStack.splice(newCursor, 1);
    newCursor--;
  }
  return { stack: newStack, cursor: newCursor };
}

export function pruneEntries(
  stack: NavEntry[],
  cursor: number,
  isStale: (entry: NavEntry) => boolean,
): { stack: NavEntry[]; cursor: number } | null {
  const kept: NavEntry[] = [];
  let newCursor = 0;
  stack.forEach((entry, i) => {
    const previous = kept[kept.length - 1];
    if (!isStale(entry) && !(previous && statesEqual(previous, entry))) kept.push(entry);
    if (i <= cursor) newCursor = Math.max(0, kept.length - 1);
  });
  if (kept.length === stack.length) return null;
  return { stack: kept, cursor: newCursor };
}

function captureState(): NavEntry {
  return {
    menuItem: activeMenuItem.value,
    settingsSubview: settingsSubview.value,
    overlay: panelOverlay.value,
    wipPreviewThreadId: wipPreviewThreadId.value,
  };
}

function migrateEntry(raw: Record<string, unknown>): NavEntry {
  if ('overlay' in raw) {
    const entry = raw as unknown as NavEntry;
    return { ...entry, settingsSubview: migrateSettingsSubview(entry.settingsSubview) };
  }
  let overlay: PanelOverlay = null;
  if (raw.inlineForm) {
    overlay = { type: 'form', form: raw.inlineForm as InlineForm };
  } else if (raw.skill || raw.app) {
    overlay = { type: 'app-ui', app: (raw.app ?? raw.skill) as import('../types').App };
  } else if (raw.filePath) {
    overlay = { type: 'file-preview', path: raw.filePath as string };
  } else if (raw.panelUrl) {
    overlay = { type: 'url-preview', url: raw.panelUrl as string };
  } else if (raw.notification) {
    overlay = { type: 'notification-detail', notification: raw.notification as import('../types').Notification };
  }
  return {
    menuItem: (raw.menuItem as MenuItem) ?? 'files',
    settingsSubview: migrateSettingsSubview(raw.settingsSubview),
    overlay,
    wipPreviewThreadId: (raw.wipPreviewThreadId as string | null | undefined) ?? null,
  };
}

function restoreState(entry: NavEntry): void {
  _restoring = true;
  try {
    const migrated = migrateEntry(entry as unknown as Record<string, unknown>);
    const menuItem = (MENU_ITEMS as readonly string[]).includes(migrated.menuItem) ? migrated.menuItem : 'files';
    activeMenuItem.value = menuItem;
    writeWindowItem('lucidos-active-menu-item', menuItem);
    settingsSubview.value = migrated.settingsSubview;
    const suppressUrlPreview = migrated.overlay?.type === 'url-preview'
      && (!inAppBrowserAvailable() || !inAppBrowserRenders(migrated.overlay.url));
    const overlay = suppressUrlPreview ? null : migrated.overlay;
    panelOverlay.value = overlay;
    if (overlay?.type === 'file-preview') {
      writeWindowItem('file-preview-open', overlay.path);
    } else {
      removeWindowItem('file-preview-open');
    }
    if (overlay?.type === 'app-ui') {
      writeWindowItem('app-window-open', overlay.app.id);
    } else {
      removeWindowItem('app-window-open');
    }
    if (overlay?.type === 'url-preview') {
      webviewInitialUrl.value = normalizeUrl(overlay.url);
    }
    wipPreviewThreadId.value = migrated.wipPreviewThreadId;
  } finally {
    _restoring = false;
  }
}

const navStack = signal<NavEntry[]>([]);
const navCursor = signal(-1);
let _restoring = false;
let _initialized = false;

function saveNavState(): void {
  try {
    writeWindowItem(NAV_KEY, JSON.stringify({
      stack: navStack.value,
      cursor: navCursor.value,
    }));
  } catch {  }
}

function ensureInitialized(): void {
  if (_initialized) return;
  _initialized = true;
  try {
    const saved = readWindowItem(NAV_KEY);
    if (saved) {
      const { stack, cursor } = JSON.parse(saved) as { stack: NavEntry[]; cursor: number };
      if (Array.isArray(stack) && stack.length > 0 && cursor >= 0 && cursor < stack.length) {
        const migrated = (stack as unknown as Record<string, unknown>[]).map(migrateEntry);
        navStack.value = migrated;
        navCursor.value = cursor;
        restoreState(migrated[cursor]);
        return;
      }
    }
  } catch {  }
  navStack.value = [captureState()];
  navCursor.value = 0;
}

export function pruneNavHistory(isStale: (entry: NavEntry) => boolean): void {
  if (_initialized) {
    const result = pruneEntries(navStack.value, navCursor.value, isStale);
    if (!result) return;
    navStack.value = result.stack.length > 0 ? result.stack : [captureState()];
    navCursor.value = result.cursor;
    saveNavState();
    return;
  }
  try {
    const saved = readWindowItem(NAV_KEY);
    if (!saved) return;
    const { stack, cursor } = JSON.parse(saved) as { stack: unknown; cursor: number };
    if (!Array.isArray(stack)) return;
    const result = pruneEntries(stack.map(migrateEntry), cursor, isStale);
    if (result) writeWindowItem(NAV_KEY, JSON.stringify(result));
  } catch {  }
}

export const canGoBack = computed(() => { ensureInitialized(); return navCursor.value > 0; });
export const canGoForward = computed(() => { ensureInitialized(); return navCursor.value < navStack.value.length - 1; });

export const navHistory = computed(() => {
  ensureInitialized();
  return { stack: navStack.value, cursor: navCursor.value };
});

export function navGoTo(index: number): void {
  ensureInitialized();
  if (index < 0 || index >= navStack.value.length) return;
  if (index !== navCursor.value) {
    navCursor.value = index;
    restoreState(navStack.value[index]);
    saveNavState();
  }
  revealContentPane();
}

export function pushNavState(): void {
  ensureInitialized();
  if (_restoring) return;
  const result = pushEntry(navStack.value, navCursor.value, captureState());
  if (result) {
    navStack.value = result.stack;
    navCursor.value = result.cursor;
    saveNavState();
  }
}

export function replaceNavState(): void {
  ensureInitialized();
  if (_restoring) return;
  if (navCursor.value < 0 || navCursor.value >= navStack.value.length) return;
  const newStack = [...navStack.value];
  newStack[navCursor.value] = captureState();
  navStack.value = newStack;
  saveNavState();
}

function foldUnpushedChange(): void {
  foldQueued = false;
  if (!_initialized) return;
  const result = foldEntry(navStack.value, navCursor.value, captureState());
  if (result) {
    navStack.value = result.stack;
    navCursor.value = result.cursor;
    saveNavState();
  }
}

let foldQueued = false;
effect(() => {
  captureState();
  if (foldQueued) return;
  foldQueued = true;
  queueMicrotask(foldUnpushedChange);
});

export function navBack(): void {
  ensureInitialized();
  if (!canGoBack.value) return;
  navCursor.value--;
  restoreState(navStack.value[navCursor.value]);
  revealContentPane();
  saveNavState();
}

export function navForward(): void {
  ensureInitialized();
  if (!canGoForward.value) return;
  navCursor.value++;
  restoreState(navStack.value[navCursor.value]);
  revealContentPane();
  saveNavState();
}
