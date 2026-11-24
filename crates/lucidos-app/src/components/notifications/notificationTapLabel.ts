import type { NavigateUi } from '@lucidos/sdk';

export function navigateTapLabel(to: NavigateUi): string {
  switch (to.target) {
    case 'dashboard':
      return 'Open dashboard';
    case 'changes':
      return 'View changes';
    case 'files':
      return 'View files';
    case 'apps':
      return 'View apps';
    case 'app-store':
      return 'Open Plugins';
    case 'triggers':
      return 'View triggers';
    case 'thread-queue':
      return 'View thread queue';
    case 'notifications':
      return 'View notifications';
    case 'settings':
      return 'Open settings';
    case 'app':
      return 'Open app';
    case 'file':
      return 'Open file';
    case 'trigger':
      return 'Open trigger';
    case 'thread':
      return 'Open thread';
    case 'new-app':
      return 'New app';
    case 'new-trigger':
      return 'New trigger';
    case 'new-chat':
      return 'New chat';
    case 'url':
      return 'Open link';
  }
  return 'Open';
}
