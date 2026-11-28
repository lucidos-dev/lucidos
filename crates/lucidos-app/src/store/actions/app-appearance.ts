import { effect } from '@preact/signals';
import { APPEARANCE_CHANNEL, THEME_MODE_ATTRIBUTE, readAppearancePush } from '@lucidos/appearance';
import { postToAppFrame } from './app-bridge';
import { mountedAppFrames } from '../../utils/appFrame';
import { appearanceVersion } from './preferences';
import { motionPreference } from '../../utils/motion';
import { themeEffectsPreference } from '../../utils/themeEffects';
import { PREF_UI_SCALE } from '@lucidos/preference-catalog';

export const WIDGET_FIT_SCALE_ATTRIBUTE = 'data-widget-fit-scale';

function paintedAppearance(): Record<string, string> {
  return readAppearancePush(
    key => localStorage.getItem(key),
    document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
  );
}

function appearanceFor(frame: HTMLIFrameElement, painted: Record<string, string>): Record<string, string> {
  const scale = frame.closest(`[${WIDGET_FIT_SCALE_ATTRIBUTE}]`)?.getAttribute(WIDGET_FIT_SCALE_ATTRIBUTE);
  return scale ? { ...painted, [PREF_UI_SCALE.key]: scale } : painted;
}

export function pushAppearanceToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, APPEARANCE_CHANNEL, appearanceFor(frame, paintedAppearance()));
}

let pushQueued = false;

function pushToEveryFrame(): void {
  pushQueued = false;
  const appearance = paintedAppearance();
  for (const frame of mountedAppFrames()) {
    postToAppFrame(frame, APPEARANCE_CHANNEL, appearanceFor(frame, appearance));
  }
}

export function installAppAppearanceSync(): void {
  effect(() => {
    void appearanceVersion.value;
    void motionPreference.value;
    void themeEffectsPreference.value;
    if (pushQueued) return;
    pushQueued = true;
    queueMicrotask(pushToEveryFrame);
  });
}
