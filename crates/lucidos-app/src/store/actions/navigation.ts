import { signal, computed } from '@preact/signals';
import {
  activeMenuItem,
  settingsSubview,
  panelOverlay,
  webviewInitialUrl,
  wipPreviewThreadId,
} from '../store';
import type { SettingsSubview, InlineForm, PanelOverlay } from '../store';
import type { MenuItem } from '../types';
import { MENU_ITEMS } from '../types';
import { normalizeUrl } from './artifacts';
import { NAV_KEY } from './entityReferences';
import { inAppBrowserAvailable } from './preferences';
import { revealContentPane } from './pane';

/** A snapshot of panel navigation state. */
export interface NavEntry {
  menuItem: MenuItem;
  settingsSubview: SettingsSubview;
  overlay: PanelOverlay;
  /** Which app coding-agent thread's worktree the app-ui iframe is serving from.
   *  Null when the iframe is showing the live workspace. Treated as nav-tracked
   *  panel state so back/forward walks each toggle and a reload restores it. */
  wipPreviewThreadId: string | null;
}

function inlineFormsEqual(a: InlineForm | null, b: InlineForm | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.type !== b.type) return false;
  switch (a.type) {
    case 'credential': {
      // Two engine-prompted requests for different services are distinct
      // overlays — without comparing `request.service`, pushNavState would
      // dedupe the second and the user could never return to it.
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
      // `sentAt` is part of the identity: the pending confirmation and the
      // receipt for the same email are different destinations (one still has a
      // Send button), so they must never dedupe against each other.
      return (
        a.sentAt === bb.sentAt &&
        ar.subject === br.subject &&
        ar.to.length === br.to.length &&
        ar.to.every((addr, i) => addr === br.to[i])
      );
    }
    case 'plugin-install':
      // Two staged installs always have distinct install_ids (UUID per call),
      // so install_id is the cheapest correct equality.
      return a.request.install_id === (b as typeof a).request.install_id;
    case 'plugin-uninstall':
      // Same as install — fresh UUID per prepare call.
      return a.request.uninstall_id === (b as typeof a).request.uninstall_id;
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

/**
 * Pure stack logic — given current stack, cursor, and new entry,
 * returns the new stack and cursor. Returns null if entry is a duplicate.
 */
export function pushEntry(
  stack: NavEntry[],
  cursor: number,
  entry: NavEntry,
): { stack: NavEntry[]; cursor: number } | null {
  if (cursor < stack.length) {
    // Same overlay = same content — skip even if menuItem differs. WIP toggle
    // is a separate axis on top of the overlay: same app-ui overlay with
    // different wipPreviewThreadId is a distinct destination (the iframe is
    // pointed at a different URL), so the overlay-only dedupe must not collapse
    // it. statesEqual catches the all-fields-match case below.
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

function captureState(): NavEntry {
  return {
    menuItem: activeMenuItem.value,
    settingsSubview: settingsSubview.value,
    overlay: panelOverlay.value,
    wipPreviewThreadId: wipPreviewThreadId.value,
  };
}

/** Migrate old NavEntry format (separate fields) to new format (overlay union).
 *  Old entries from localStorage may have inlineForm/app/filePath/etc. instead of overlay. */
function migrateEntry(raw: Record<string, unknown>): NavEntry {
  if ('overlay' in raw) return raw as unknown as NavEntry;
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
    settingsSubview: (raw.settingsSubview as SettingsSubview) ?? 'main',
    overlay,
    wipPreviewThreadId: (raw.wipPreviewThreadId as string | null | undefined) ?? null,
  };
}

/** Whether a panel overlay is a transient, request-backed form that must NOT be
 *  resurrected from the persisted nav stack on reload. These overlays are backed
 *  by an engine-staged request whose id (a fresh-per-prepare-call UUID) is dead
 *  the moment the page reloads — restoring the panel would show a stale form
 *  whose Confirm/Cancel resolves a request that no longer exists. Covers plugin
 *  install/uninstall, a *pending* email-confirm, and an *engine-prompted*
 *  credential request (a `credential` form WITH a `request`). A `credential`
 *  form WITHOUT a request is a Settings-driven edit of a stored credential and
 *  restores normally, as do the persistent forms (app-edit, new-app, trigger).
 *  Mirrors the url-preview suppression guard in `restoreState`. */
function isTransientForm(overlay: PanelOverlay): boolean {
  if (overlay?.type !== 'form') return false;
  const form = overlay.form;
  switch (form.type) {
    case 'plugin-install':
    case 'plugin-uninstall':
      return true;
    case 'email-confirm':
      // A SENT email is a receipt, not a staged request — nothing is left to
      // resolve, so it restores like any other destination. That's what makes it
      // a real history entry rather than a session artifact. Only the pending
      // confirmation dies with the page.
      return form.sentAt == null;
    case 'credential':
      return form.request != null;
    default:
      return false;
  }
}

function restoreState(entry: NavEntry): void {
  _restoring = true;
  try {
    const migrated = migrateEntry(entry as unknown as Record<string, unknown>);
    // Validate menuItem — old nav stack entries may contain removed values like 'pinned'
    const menuItem = (MENU_ITEMS as readonly string[]).includes(migrated.menuItem) ? migrated.menuItem : 'files';
    activeMenuItem.value = menuItem;
    localStorage.setItem('lucidos-active-menu-item', menuItem);
    settingsSubview.value = migrated.settingsSubview;
    // Don't restore a url-preview overlay when the in-app browser isn't the
    // active path: in Chrome/PWA it uses a broken iframe, and under Tauri the
    // experimental in-app webview is off by default (URLs open in the system
    // browser, so there's no panel to resurrect on reload).
    const suppressUrlPreview = migrated.overlay?.type === 'url-preview'
      && !inAppBrowserAvailable();
    // Drop any transient, request-backed form overlay (plugin install/uninstall,
    // email-confirm, engine-prompted credential) — its staged request is dead
    // after a reload, so resurrecting the panel would show a stale form (see
    // isTransientForm). Persistent forms (app-edit, new-app, trigger, credential
    // edit) still restore. Both guards resolve to null so the localStorage side
    // effects below key off the final overlay value.
    const overlay = suppressUrlPreview || isTransientForm(migrated.overlay)
      ? null : migrated.overlay;
    panelOverlay.value = overlay;
    if (overlay?.type === 'file-preview') {
      localStorage.setItem('file-preview-open', overlay.path);
    } else {
      localStorage.removeItem('file-preview-open');
    }
    if (overlay?.type === 'app-ui') {
      localStorage.setItem('app-window-open', overlay.app.id);
    } else {
      localStorage.removeItem('app-window-open');
    }
    if (overlay?.type === 'url-preview') {
      webviewInitialUrl.value = normalizeUrl(overlay.url);
    }
    // Restore WIP last so the wipPreview effect re-runs with the overlay
    // (and therefore currentApp) already in place — otherwise the effect
    // would briefly see WIP set while the iframe is still showing the prior
    // app and prematurely clear WIP via the openApp.id !== wipAppId branch.
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
    localStorage.setItem(NAV_KEY, JSON.stringify({
      stack: navStack.value,
      cursor: navCursor.value,
    }));
  } catch { /* localStorage full or unavailable — non-critical */ }
}

function ensureInitialized(): void {
  if (_initialized) return;
  _initialized = true;
  try {
    const saved = localStorage.getItem(NAV_KEY);
    if (saved) {
      const { stack, cursor } = JSON.parse(saved) as { stack: NavEntry[]; cursor: number };
      if (Array.isArray(stack) && stack.length > 0 && cursor >= 0 && cursor < stack.length) {
        // Migrate old-format entries (separate fields) to new format (overlay union)
        const migrated = (stack as unknown as Record<string, unknown>[]).map(migrateEntry);
        navStack.value = migrated;
        navCursor.value = cursor;
        restoreState(migrated[cursor]);
        return;
      }
    }
  } catch { /* corrupt data — fall through to fresh init */ }
  navStack.value = [captureState()];
  navCursor.value = 0;
}

export const canGoBack = computed(() => { ensureInitialized(); return navCursor.value > 0; });
export const canGoForward = computed(() => { ensureInitialized(); return navCursor.value < navStack.value.length - 1; });

/** The full panel-nav stack + cursor — read by the long-press history menu on
 *  the content chevrons to list the destinations back/forward walk toward. */
export const navHistory = computed(() => {
  ensureInitialized();
  return { stack: navStack.value, cursor: navCursor.value };
});

/** Jump straight to an absolute index in the panel-nav stack (the long-press
 *  history menu). Out-of-range is a no-op; re-selecting the current entry just
 *  re-reveals the content pane. */
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

/** Overwrite the entry at the cursor with the current state — used when a
 *  panel already on screen mutates in place (e.g. switching files inside the
 *  diff split-view) and we want a single history slot for the whole session. */
export function replaceNavState(): void {
  ensureInitialized();
  if (_restoring) return;
  if (navCursor.value < 0 || navCursor.value >= navStack.value.length) return;
  const newStack = [...navStack.value];
  newStack[navCursor.value] = captureState();
  navStack.value = newStack;
  saveNavState();
}

export function navBack(): void {
  ensureInitialized();
  if (!canGoBack.value) return;
  navCursor.value--;
  restoreState(navStack.value[navCursor.value]);
  // Back/Forward are user-intent content navigation (the buttons live in the
  // content-pane header / app header), so make the restored content visible —
  // same contract as switchMenuItem/openApp. Mobile: no-op when already on the
  // content pane; desktop: re-expands a collapsed split. NOT called from
  // restoreState itself, since that also runs on page-load init where a forced
  // reveal would override the reloaded pane state.
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
