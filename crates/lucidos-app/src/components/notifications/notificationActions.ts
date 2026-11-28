import type { NavigateUi, Tap } from '@lucidos/sdk';
import type { Notification } from '../../store/types';

export interface NotificationActions {
  openThread: boolean;
  discuss: boolean;
  openTrigger: boolean;
  navTap: NavigateUi | null;
}

export function notificationTriggerId(n: Pick<Notification, 'task_id'>): string | null {
  return n.task_id ?? null;
}

export function notificationActions(
  n: Pick<Notification, 'task_id' | 'thread_id'> & { tap?: Tap },
  appLinked: boolean,
): NotificationActions {
  const openThread = !!n.thread_id;
  const openTrigger = !!notificationTriggerId(n);
  const tapTo = n.tap?.kind === 'navigate' ? n.tap.to : null;
  const duplicatesDedicated =
    (tapTo?.target === 'thread' && openThread) ||
    (tapTo?.target === 'trigger' && openTrigger) ||
    (tapTo?.target === 'app' && appLinked);
  const navTap = tapTo && !duplicatesDedicated ? tapTo : null;
  const reachesThread = openThread || tapTo?.target === 'thread';
  return {
    openThread,
    discuss: !reachesThread,
    openTrigger,
    navTap,
  };
}
