import { activeMenuItem, panelOverlay, activeInlineForm, panelUrl, panelTitle, settingsSubview, settingsSubviewLabel, triggers, appsList, parseRepoPath, repoPending, selectedChange, wipPreviewThreadId, threadMap } from '../../store/store';
import { CODING_AGENT_CHANNEL, type ThreadChannel, type InlineForm } from '../../store/store';
import type { NavEntry } from '../../store/actions/navigation';
import { loadedOr } from '../../store/types';
import { formatChannel } from '../../utils/formatChannel';
import { PENDING_TITLE_PLACEHOLDER } from '../../store/thread-events';

const menuLabels: Record<string, string> = {
  files: 'Files', apps: 'Apps', plugins: 'Plugins', triggers: 'Triggers',
  changes: 'Changes', notifications: 'Notifications',
  settings: 'Settings',
};

export const CHANNEL_OPTIONS: { value: ThreadChannel; label: string }[] = [
  { value: 'chat', label: formatChannel('chat') },
  { value: CODING_AGENT_CHANNEL, label: 'Coding Agent' },
  { value: 'trigger', label: formatChannel('trigger') },
];

function getFormTitle(form: InlineForm): string {
  switch (form.type) {
    case 'trigger': {
      if (!form.triggerId) return 'New Trigger';
      const trigger = loadedOr(triggers.value, []).find(t => t.id === form.triggerId);
      return trigger?.name || 'Trigger';
    }
    case 'app-edit': {
      const app = loadedOr(appsList.value, []).find(s => s.id === form.appId);
      return app?.name || 'Edit App';
    }
    case 'new-app': return 'New App';
    case 'credential': return form.editing ? 'Edit Credential' : 'Add Credential';
    // Also the nav-history row's label (via navEntryTitle), so a sent email
    // reads as the receipt it is rather than a confirmation still pending.
    case 'email-confirm': return form.sentAt ? 'Email Sent' : 'Confirm Email';
    case 'plugin-install': return `Install ${form.request.plugin_name}`;
    case 'plugin-uninstall': return `Uninstall ${form.request.plugin_name}`;
  }
}

function getHostname(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

/** Base name of a file-preview overlay path. A repo-encoded path
 *  (`repo:<repoId>:file:<path>`) is unwrapped first — a repo file at the clone
 *  root has no `/` to split on, so the raw encoding would surface the repo id
 *  and mode as the header title. */
function previewFileName(path: string): string {
  const repoRelative = parseRepoPath(path)?.path ?? path;
  return repoRelative.split('/').pop() || repoRelative;
}

export function getContentTitle(): string {
  const overlay = panelOverlay.value;
  const form = activeInlineForm.value;
  const url = panelUrl.value;
  const pageTitle = panelTitle.value;
  const active = activeMenuItem.value;

  if (overlay?.type === 'form') return getFormTitle(form!);
  if (overlay?.type === 'app-ui') {
    const wipTid = wipPreviewThreadId.value;
    if (wipTid) {
      const wipTitle = threadMap.value.get(wipTid)?.meta.title;
      if (wipTitle && wipTitle !== PENDING_TITLE_PLACEHOLDER) {
        return `${overlay.app.name} (WIP by ${wipTitle})`;
      }
      if (wipTitle) return `${overlay.app.name} (WIP)`;
    }
    return overlay.app.name;
  }
  if (overlay?.type === 'file-preview') {
    const fileName = previewFileName(overlay.path);
    const desc = getDiffDescription();
    if (desc) return `${fileName} — ${desc}`;
    return fileName;
  }
  if (overlay?.type === 'url-preview') return pageTitle || getHostname(url!);
  if (overlay?.type === 'notification-detail') {
    return overlay.notification.title ? `Notification - ${overlay.notification.title}` : 'Notification';
  }
  if (!overlay && active === 'settings' && settingsSubview.value !== 'main') {
    return settingsSubviewLabel(settingsSubview.value) || '';
  }
  return menuLabels[active] || '';
}

/** Title for an arbitrary captured `NavEntry` — the rows in the back/forward
 *  long-press history menu. Mirrors `getContentTitle` but reads the entry's own
 *  fields instead of the live signals (so past entries label correctly), and
 *  falls back to the host name for url-preview entries since the live page
 *  <title> isn't captured in history. */
export function navEntryTitle(entry: NavEntry): string {
  const overlay = entry.overlay;
  if (overlay?.type === 'form') return getFormTitle(overlay.form);
  if (overlay?.type === 'app-ui') {
    const wip = entry.wipPreviewThreadId;
    if (wip) {
      const wipTitle = threadMap.value.get(wip)?.meta.title;
      if (wipTitle && wipTitle !== PENDING_TITLE_PLACEHOLDER) return `${overlay.app.name} (WIP by ${wipTitle})`;
      if (wipTitle) return `${overlay.app.name} (WIP)`;
    }
    return overlay.app.name;
  }
  if (overlay?.type === 'file-preview') return previewFileName(overlay.path);
  if (overlay?.type === 'url-preview') return getHostname(overlay.url);
  if (overlay?.type === 'notification-detail') return overlay.notification.title || 'Notification';
  if (!overlay && entry.menuItem === 'settings' && entry.settingsSubview !== 'main') {
    return settingsSubviewLabel(entry.settingsSubview) || 'Settings';
  }
  return menuLabels[entry.menuItem] || entry.menuItem;
}

/** Content-pane category for a captured `NavEntry` — drives the icon shown
 *  beside each row in the content back/forward history menu (PanelNav). Mirrors
 *  `navEntryTitle`'s branching, mapping every destination to one of the
 *  Search Everywhere content categories (plus the content-pane-only ones:
 *  `notifications`, `web`). The bare menu items already share their names with
 *  the category icons, so the no-overlay case returns the menu item verbatim;
 *  anything unmapped falls through to the icon's default. */
export function navEntryCategory(entry: NavEntry): string {
  const overlay = entry.overlay;
  if (overlay?.type === 'form') {
    switch (overlay.form.type) {
      case 'trigger': return 'triggers';
      case 'app-edit':
      case 'new-app': return 'apps';
      case 'credential':
      case 'email-confirm':
      case 'plugin-install':
      case 'plugin-uninstall': return 'settings';
    }
  }
  if (overlay?.type === 'app-ui') return 'apps';
  if (overlay?.type === 'file-preview') return 'files';
  if (overlay?.type === 'url-preview') return 'web';
  if (overlay?.type === 'notification-detail') return 'notifications';
  // No overlay → the active menu item is the destination; its name is also the
  // category icon's key (files / apps / triggers / settings / changes /
  // notifications).
  return entry.menuItem;
}

export function getDiffDescription(): string | null {
  const overlay = panelOverlay.value;
  if (overlay?.type !== 'file-preview') return null;
  const parsed = parseRepoPath(overlay.path);
  if (parsed?.mode !== 'diff') return null;
  const desc = repoPending.value?.description
    ?? selectedChange.value?.description;
  return desc || null;
}
