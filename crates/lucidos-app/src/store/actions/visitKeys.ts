/** One name for one place, shared by the two sides that must agree about it.
 *
 *  A notification's tap names a destination as a `NavigateUi`. The shell names
 *  where the reader currently is from its own signals. The *seen target* rule
 *  (`notification-visit.ts`) compares the two, so a place they spell differently
 *  is a notification that never clears. The constructors below are the only
 *  place either spelling is written, which is what makes them agree.
 *
 *  See `system-knowhow/notifications.md` §4 and
 *  `docs/plans/2026-08-30-visiting-a-tap-target-marks-its-notifications-read.md`.
 */

import type { Tap } from '@lucidos/sdk';
import { aliasRetiredSettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { resolveFileTarget } from './fileTarget';

export const threadVisitKey = (id: string): string => `thread:${id}`;
export const appVisitKey = (appId: string): string => `app:${appId}`;
/** `path` is the RESOLVED locator, i.e. what `resolveFileTarget` returns and
 *  what the `file-preview` overlay stores. A raw `file_path` off a tap can be
 *  either un-normalized or repo-encoded, so both sides resolve first. */
export const fileVisitKey = (path: string): string => `file:${path}`;
export const triggerVisitKey = (id: string): string => `trigger:${id}`;
export const settingsVisitKey = (view: string): string => `settings:${view}`;
export const panelVisitKey = (item: MenuItem): string => `panel:${item}`;

/** The navigable shape both `handleNavigationRequest` and a stored tap carry.
 *  Loose about `target` on purpose: `app-ui` is a historical alias no longer in
 *  `NavigateTarget`, and an old notification row can still hold it. */
export interface NavigateLike {
  target: string;
  settings_view?: string;
  app_id?: string;
  file_path?: string;
  id?: string;
  event_id?: string;
}

/** The place a navigation lands, or null when it lands nowhere revisitable.
 *
 *  Null for three shapes, each for its own reason. A `url` leaves the app
 *  entirely. `new-chat` / `new-app` / `new-trigger` create something, so there
 *  is nothing to come back to. A target missing the id it needs cannot name a
 *  place at all.
 *
 *  Detail INSIDE a place is deliberately dropped: an app's fragment, a file's
 *  line, a plugin row's id. Those pick a spot within a thing the reader is
 *  already looking at, and the rule is about the thing.
 *
 *  `navigate_ui` grows targets over time, and a new one landing here silently as
 *  `null` would just mean its notifications never clear. `visitKeys.test.ts`
 *  walks the generated `NAVIGATE_TARGETS` and fails on a target this function
 *  has not been taught. A new one has to say which place it is. */
export function navigateVisitKey(nav: NavigateLike): string | null {
  switch (nav.target) {
    case 'thread':
      return nav.id ? threadVisitKey(nav.id) : null;
    case 'app':
    case 'app-ui':
      return nav.app_id ? appVisitKey(nav.app_id) : null;
    case 'file':
      return nav.file_path ? fileVisitKey(resolveFileTarget(nav.file_path).path) : null;
    case 'trigger':
      return nav.id ? triggerVisitKey(nav.id) : null;
    case 'settings':
      // A bare `settings` lands on the home list, which IS a sub-section here
      // (`settingsSubview` calls it `main`), so it gets a key like any other.
      return settingsVisitKey(
        nav.settings_view ? aliasRetiredSettingsSubview(nav.settings_view) : 'main',
      );
    case 'thread-queue':
      // The router reinterprets this one onto the System subpanel rather than a
      // top-level panel, so the key has to follow it there.
      return settingsVisitKey('thread-queue');
    case 'app-store':
    case 'plugins':
      // Both land on Plugins. `app-store` only differs by the All | Installed
      // toggle, which is a filter on one panel, not a second place.
      return panelVisitKey('plugins');
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      return panelVisitKey(nav.target);
    default:
      return null;
  }
}

/** What a notification points at, in the form the seen rule can measure. */
export type SeenTarget =
  | { kind: 'event'; threadId: string; eventId: string }
  | { kind: 'place'; key: string };

/** Resolve a notification's tap to the thing the reader has to look at.
 *
 *  An `event` target is the strict case, and the common one. The reader has
 *  seen it when that event's own card is in the transcript's visible band. A
 *  `place` target has no card to measure, so being there is the whole test.
 *
 *  Null for a `modal` tap, which has no destination outside the notification
 *  detail. Opening that detail already marks the row read, so there is nothing
 *  for this rule to add. Null too for a navigate that names no revisitable
 *  place.
 *
 *  Deliberately reads ONLY `tap`. A notification's own `thread_id` is
 *  provenance, not intent. The engine stamps it on every `send_notification`
 *  from the origin thread. Keying on it would clear a trigger's daily summary
 *  the moment the reader opened that transcript. See
 *  `system-knowhow/notifications.md` §4, "Why `event_id` and not
 *  `thread_id`?". */
export function notificationTarget(tap: Tap | null | undefined): SeenTarget | null {
  if (!tap || tap.kind !== 'navigate') return null;
  const to = tap.to;
  if (to.target === 'thread' && to.id && to.event_id) {
    return { kind: 'event', threadId: to.id, eventId: to.event_id };
  }
  const key = navigateVisitKey(to);
  return key ? { kind: 'place', key } : null;
}
