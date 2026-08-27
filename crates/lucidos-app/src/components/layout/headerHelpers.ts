import { activeMenuItem, panelOverlay, activeInlineForm, panelUrl, panelTitle, settingsSubview, settingsSubviewLabel, settingsSubviewShortLabel, triggers, appsList, parseRepoPath, repoPending, selectedChange, wipPreviewThreadId, threadMap } from '../../store/store';
import { CODING_AGENT_CHANNEL, type ThreadChannel, type InlineForm, type SettingsNavKey } from '../../store/store';
import type { NavEntry } from '../../store/actions/navigation';
import { loadedOr } from '../../store/types';
import { formatChannel } from '../../utils/formatChannel';
import { previewFileName } from '../../utils/previewPath';
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
    // Past tense once the panel is a receipt, so the nav-history row says what
    // happened rather than offering an action that is already resolved.
    case 'plugin-install':
      return `${form.installed ? 'Installed' : 'Install'} ${form.request.plugin_name}`;
    case 'plugin-uninstall':
      return `${form.removed ? 'Uninstalled' : 'Uninstall'} ${form.request.plugin_name}`;
  }
}

function getHostname(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

/** Title of whatever the content pane is showing, in one of two forms.
 *
 *  `short` is what the header BAR renders, and the rule for it is: a title WE
 *  author says the destination's kind and nothing more, because the bar is the
 *  narrowest surface a title ever appears on. Everything appended to name the
 *  particular instance (which notification, which change a diff belongs to,
 *  which thread is previewing an app) belongs to the full form, which the
 *  title's own tap-tooltip carries and the back/forward history menu renders.
 *  A name we did NOT author (a file, an app, a web page, a thread) has no short
 *  form to author, so the ellipsis stays the net underneath.
 *
 *  Both forms come out of this one function, on purpose: a new destination
 *  cannot land in the full form and go missing from the short one. */
function contentTitle(short: boolean): string {
  const settingsLabel: (key: SettingsNavKey) => string | undefined =
    short ? settingsSubviewShortLabel : settingsSubviewLabel;
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
      // Which thread is previewing it is instance detail: the bar says the app
      // is showing work in progress, the tooltip says whose.
      if (wipTitle && wipTitle !== PENDING_TITLE_PLACEHOLDER && !short) {
        return `${overlay.app.name} (WIP by ${wipTitle})`;
      }
      if (wipTitle) return `${overlay.app.name} (WIP)`;
    }
    return overlay.app.name;
  }
  if (overlay?.type === 'file-preview') {
    const fileName = previewFileName(overlay.path);
    // The change a diff belongs to is a sentence, not a title. The bar names the
    // file; the tooltip is already the description on its own (see AppHeader,
    // which prefers `getDiffDescription`), so nothing is lost by dropping it.
    const desc = short ? null : getDiffDescription();
    if (desc) return `${fileName}: ${desc}`;
    return fileName;
  }
  if (overlay?.type === 'url-preview') return pageTitle || getHostname(url!);
  if (overlay?.type === 'notification-detail') {
    if (short || !overlay.notification.title) return 'Notification';
    return `Notification - ${overlay.notification.title}`;
  }
  if (!overlay && active === 'settings' && settingsSubview.value !== 'main') {
    return settingsLabel(settingsSubview.value) || '';
  }
  return menuLabels[active] || '';
}

/** The full title, for every surface with room for it: the tap-tooltip on the
 *  header title, and anything else that wants the destination's real name. */
export function getContentTitle(): string {
  return contentTitle(false);
}

/** The title as the header bar renders it: the destination's kind, without the
 *  detail that names the particular instance. See `contentTitle`. */
export function getContentTitleShort(): string {
  return contentTitle(true);
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
 *  beside each row in the content back/forward history menu (ContentNav). Mirrors
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
      case 'email-confirm': return 'settings';
      // A plugin panel is about a plugin, and `plugins` is both a menu item and
      // a CategoryIcon key, so the row carries the plugin glyph rather than the
      // settings cog it used to borrow.
      case 'plugin-install':
      case 'plugin-uninstall': return 'plugins';
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
