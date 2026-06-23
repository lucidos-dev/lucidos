import type { NavigateUi } from '@lucidos/sdk';

/** Button label for a `navigate`-kind notification tap, shown in the
 *  notification detail panel (`NotificationDetailInline`).
 *
 *  A navigate tap is actionable from the OS-push tap and the in-app toast
 *  (both run `handleNavigationRequest`), but the detail panel would otherwise
 *  ignore the `tap` field entirely — so a navigate notification (e.g. the
 *  "N changes ready to apply" trigger push, which taps to the Changes panel)
 *  would be a dead end when read from the bell. This label gives that button
 *  its text.
 *
 *  Targets mirror the SDK `NavigateTarget` union. `thread` / `app` are usually
 *  served by the panel's dedicated "Open thread" / "Open <app>" buttons, but
 *  this still labels them for a navigate tap that lacks the matching row
 *  metadata. The trailing fallback guards a target string the engine added
 *  after this build — `validateTap` doesn't constrain the target value. */
export function navigateTapLabel(to: NavigateUi): string {
  switch (to.target) {
    case 'changes':
      return 'View changes';
    case 'files':
      return 'View files';
    case 'apps':
      return 'View apps';
    case 'app-store':
      return 'Open app store';
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
