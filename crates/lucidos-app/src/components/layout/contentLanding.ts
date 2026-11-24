import { settingsScrollTarget } from '../../store/store';
import { navFocusElement } from '../shared/focusMarker';
import { settingsViewKey } from './contentViewKey';

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

function isSettingsSubview(viewKey: string | null): boolean {
  return viewKey !== null && viewKey.startsWith('settings:') && viewKey !== settingsViewKey('main');
}
