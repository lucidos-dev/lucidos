import type { NavigateUi, Tap } from '@lucidos/sdk';
import type { Notification } from '../../store/types';

/** Which action buttons the notification detail panel offers, for one
 *  notification. Pure so the dedup rule below is unit-testable: the panel
 *  itself only renders what this returns. */
export interface NotificationActions {
  /** "Open thread": the originating thread. */
  openThread: boolean;
  /** "Discuss": start a chat with this notification quoted, and send it.
   *  Offered exactly when nothing here reaches a thread, because a thread the
   *  row already points at IS the discussion. So the row always has at least one
   *  button, which is why the panel renders the actions row unconditionally. */
  discuss: boolean;
  /** "Open trigger": the trigger this notification is about. */
  openTrigger: boolean;
  /** "View changes" / "Open settings" / etc: a `navigate`-kind tap that no
   *  dedicated button above already covers. `null` when there is none. */
  navTap: NavigateUi | null;
}

/** The trigger a notification is about, or `null`.
 *
 *  `task_id` is the trigger's id: the column name predates the trigger
 *  vocabulary, and the engine sets it only on a trigger-failure notification
 *  (`scheduler::user_tasks::emit_failure_notification`). The fix for a failed
 *  run almost always lives in that trigger's settings: the cron, the intent, or
 *  the side-effect grant a command-guard block is asking for. */
export function notificationTriggerId(n: Pick<Notification, 'task_id'>): string | null {
  return n.task_id ?? null;
}

/** Decide the panel's action buttons.
 *
 *  A `navigate` tap (e.g. the "N changes ready to apply" trigger push, which
 *  taps to the Changes panel) is actionable from the OS-push tap and the in-app
 *  toast, but the inbox detail would otherwise ignore the tap entirely, so it
 *  gets a button here too. It is dropped when a dedicated button already covers
 *  the same destination, so the panel never shows two buttons that do the same
 *  thing.
 *
 *  `appLinked` is an input rather than an output: the panel has to narrow its
 *  own `LinkedAppResult` to render "Open <app>" anyway, so returning the same
 *  boolean would only make the caller test it twice. It still participates here
 *  because the app-tap dedup depends on it. */
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
  // A row reaches a thread two ways: its own `thread_id` column, or a
  // thread-targeted tap, which survives the dedup above precisely when the
  // column is empty. `navigateTapLabel` renders that tap as "Open thread" too,
  // so reading only the column put Discuss next to a button of that name.
  const reachesThread = openThread || tapTo?.target === 'thread';
  return {
    openThread,
    discuss: !reachesThread,
    openTrigger,
    navTap,
  };
}
