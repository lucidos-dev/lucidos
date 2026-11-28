import type { InlineForm, PanelOverlay, SettingsSubview } from '../../store/store';

function digest(text: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16);
}

export function inlineFormKey(form: InlineForm): string {
  switch (form.type) {
    case 'credential':
      if (form.editing) return `credential:edit:${form.editing}`;
      return `credential:new:${form.request?.service ?? ''}`;
    case 'app-edit': return `app-edit:${form.appId}`;
    case 'new-app': return 'new-app';
    case 'trigger': return `trigger:${form.triggerId ?? 'new'}`;
    case 'email-confirm':
      return `email-confirm:${digest([form.request.account, form.request.to.join(','), form.request.subject].join('|'))}`;
    case 'plugin-install': return `plugin-install:${form.request.install_id}`;
    case 'plugin-uninstall': return `plugin-uninstall:${form.request.uninstall_id}`;
  }
}

export function settingsViewKey(subview: SettingsSubview): string {
  return `settings:${subview}`;
}

export function contentViewKey(
  active: string | null,
  overlay: PanelOverlay,
  settingsSubview: SettingsSubview,
): string | null {
  if (overlay) {
    if (overlay.type === 'form') return `form:${inlineFormKey(overlay.form)}`;
    if (overlay.type === 'file-preview') return `file:${overlay.path}`;
    if (overlay.type === 'url-preview') return `url:${overlay.url}`;
    if (overlay.type === 'notification-detail') return `notification:${overlay.notification.id}`;
    if (overlay.type === 'terminal') return `terminal:${overlay.threadId}`;
    if (overlay.type === 'plugin-detail') return `plugin:${overlay.marketplaceId}:${overlay.pluginId}`;
    return overlay.type;
  }
  if (active === 'settings') return settingsViewKey(settingsSubview);
  return active;
}
