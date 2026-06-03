import { activeMenuItem, panelOverlay, activeInlineForm, panelUrl, panelTitle, settingsSubview, SETTINGS_NAV_ITEMS, triggers, appsList, parseRepoPath, repoPending, selectedChange, wipPreviewThreadId, threadMap } from '../../store/store';
import type { ThreadChannel, InlineForm } from '../../store/store';
import { loadedOr } from '../../store/types';
import { formatChannel } from '../../utils/formatChannel';
import { PENDING_TITLE_PLACEHOLDER } from '../../store/thread-events';

const menuLabels: Record<string, string> = {
  files: 'Files', apps: 'Apps', triggers: 'Triggers',
  changes: 'Changes', notifications: 'Notifications',
  settings: 'Settings',
};

export const CHANNEL_OPTIONS: { value: ThreadChannel; label: string }[] = [
  { value: 'chat', label: formatChannel('chat') },
  { value: 'claude_code', label: formatChannel('claude_code') },
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
    case 'email-confirm': return 'Confirm Email';
    case 'plugin-install': return `Install ${form.request.plugin_name}`;
    case 'plugin-uninstall': return `Uninstall ${form.request.plugin_name}`;
  }
}

function getHostname(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
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
    const fileName = overlay.path.split('/').pop() || '';
    const desc = getDiffDescription();
    if (desc) return `${fileName} — ${desc}`;
    return fileName;
  }
  if (overlay?.type === 'url-preview') return pageTitle || getHostname(url!);
  if (!overlay && active === 'settings' && settingsSubview.value !== 'main') {
    return SETTINGS_NAV_ITEMS.find(i => i.key === settingsSubview.value)?.label || '';
  }
  return menuLabels[active] || '';
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
