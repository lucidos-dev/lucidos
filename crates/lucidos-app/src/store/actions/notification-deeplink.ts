/** Pure resolver for notification deep-links.
 *
 *  A push tap can carry an `app_id` (open the linked app) and/or a
 *  `notification_id` (open the inbox modal). When neither is set, the resolver
 *  returns `noop` — there is nothing actionable to do.
 *
 *  Intentionally does NOT navigate to the source thread, even when the push
 *  payload contains one. Auto-jumping to the conversation that emitted the
 *  notification yanked the user away from whatever they were doing; the
 *  notification still appears in the inbox so the user can navigate manually
 *  if they want to.
 */
export type DeepLinkAction =
  | { type: 'open-app'; id: string }
  | { type: 'view-notification'; id: string }
  | { type: 'noop' };

export interface DeepLinkTarget {
  app?: string | null;
  notification?: string | null;
}

export function resolveDeepLink(target: DeepLinkTarget): DeepLinkAction {
  if (target.app) return { type: 'open-app', id: target.app };
  if (target.notification) return { type: 'view-notification', id: target.notification };
  return { type: 'noop' };
}
