import { settingsScrollTarget } from '../../store/store';
import { navFocusElement } from '../shared/focusMarker';
import { settingsViewKey } from './contentViewKey';

/** Answers whether a navigation landing owns the content pane's scroll
 *  position, so the pane's scroll memory must not place the reader.
 *
 *  Two things claim it. One is a settings scroll target on a settings view,
 *  which `SettingsView` spends on its first render there. The other is a row a
 *  landing marked inside `body`. Child effects run first, so a settings landing
 *  usually runs before the scroll memory attaches and leaves only its marker.
 *
 *  A landing holds its claim for the rest of that view's visit. The scroll
 *  memory asks again when its dead-link rescue fires, and by then the reader
 *  may have dismissed the marker without scrolling.
 *
 *  The plugin and trigger targets claim nothing while they wait. They wait on
 *  data and can linger, such as a plugin row a search filter hides, and the
 *  view would then never be placed. Their landing claims the view once it
 *  marks the row. */
export function createContentLandingClaim(): (viewKey: string | null, body: HTMLElement | null) => boolean {
  let landedOn: string | null = null;
  return (viewKey, body) => {
    if (landedOn !== viewKey) landedOn = null;
    const marked = navFocusElement();
    if (marked !== null && body !== null && body.contains(marked)) landedOn = viewKey;
    if (landedOn !== null) return true;
    return isSettingsSubview(viewKey) && settingsScrollTarget.value !== null;
  };
}

/** Settings home has no anchors, and `SettingsView` leaves the target unspent
 *  there, so only a sub-section's view counts. */
function isSettingsSubview(viewKey: string | null): boolean {
  return viewKey !== null && viewKey.startsWith('settings:') && viewKey !== settingsViewKey('main');
}
