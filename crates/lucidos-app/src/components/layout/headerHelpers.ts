import { activeMenuItem, panelOverlay, activeInlineForm, panelUrl, panelTitle, settingsSubview, SETTINGS_NAV_ITEMS, triggers, appsList, threadChannelFilter, excludedTriggerIds, parseRepoPath, repoPending, selectedChange } from '../../store/store';
import type { ThreadChannel, InlineForm } from '../../store/store';
import { loadedOr } from '../../store/types';
import { formatChannel } from '../../utils/formatChannel';

export const menuLabels: Record<string, string> = {
  files: 'Files', apps: 'Apps', triggers: 'Triggers',
  changes: 'Changes', notifications: 'Notifications',
  settings: 'Settings',
};

export const CHANNEL_OPTIONS: { value: ThreadChannel; label: string }[] = [
  { value: 'chat', label: formatChannel('chat') },
  { value: 'claude_code', label: formatChannel('claude_code') },
  { value: 'trigger', label: formatChannel('trigger') },
];

export function getFormTitle(form: InlineForm): string {
  switch (form.type) {
    case 'trigger': {
      if (!form.taskId) return 'New Trigger';
      const task = loadedOr(triggers.value, []).find(t => t.id === form.taskId);
      return task?.name || 'Trigger';
    }
    case 'app-edit': {
      const app = loadedOr(appsList.value, []).find(s => s.id === form.appId);
      return app?.name || 'Edit App';
    }
    case 'new-app': return 'New App';
    case 'credential': return form.editing ? 'Edit Credential' : 'Add Credential';
    case 'email-confirm': return 'Confirm Email';
  }
}

export function getHostname(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

export function getContentTitle(): string {
  const overlay = panelOverlay.value;
  const form = activeInlineForm.value;
  const url = panelUrl.value;
  const pageTitle = panelTitle.value;
  const active = activeMenuItem.value;

  if (overlay?.type === 'form') return getFormTitle(form!);
  if (overlay?.type === 'app-ui') return overlay.app.name;
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

export function toggleChannel(channel: ThreadChannel) {
  const current = threadChannelFilter.value;
  const next = new Set(current);
  if (next.has(channel)) {
    if (next.size > 1) next.delete(channel);
  } else {
    next.add(channel);
  }
  threadChannelFilter.value = next;
  localStorage.setItem('lucidos-thread-channel-filter', JSON.stringify([...next]));
}

function persistExcludedTriggerIds(set: Set<string>) {
  excludedTriggerIds.value = set;
  localStorage.setItem('lucidos-excluded-trigger-ids', JSON.stringify([...set]));
}

/** Toggle whether a specific trigger's threads are visible. The default state
 *  is "shown" (excluded set is empty), so toggling moves the ID in/out of the
 *  exclusion set. */
export function toggleTriggerId(triggerId: string) {
  const next = new Set(excludedTriggerIds.value);
  if (next.has(triggerId)) {
    next.delete(triggerId);
  } else {
    next.add(triggerId);
  }
  persistExcludedTriggerIds(next);
}

/** Show every trigger by clearing the exclusion set. */
export function showAllTriggers() {
  if (excludedTriggerIds.value.size === 0) return;
  persistExcludedTriggerIds(new Set());
}

/** Hide every known trigger by adding all current trigger IDs to the
 *  exclusion set. Triggers created later will appear by default — matching
 *  the channel filter's "everything is included unless explicitly hidden"
 *  semantics. */
export function hideAllTriggers(triggerIds: readonly string[]) {
  if (triggerIds.length === 0) return;
  persistExcludedTriggerIds(new Set(triggerIds));
}
