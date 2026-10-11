import { effect } from '@preact/signals';
import { APPEARANCE_CHANNEL, readAppearancePush } from '@lucidos/appearance';
import { postToAppFrame } from './app-bridge';
import { mountedAppFrames } from '../../utils/appFrame';
import { appearanceVersion } from './preferences';
import { motionPreference } from '../../utils/motion';
import { themeEffectsPreference } from '../../utils/themeEffects';

/** What the shell painted, read from the mirrors each paint writes. The SDK's
 *  `watchPreferences()` repaints the app from it, with no request, so the app
 *  moves with the shell and not after the debounced save. */
function paintedAppearance(): Record<string, string> {
  return readAppearancePush(key => localStorage.getItem(key));
}

/** Hand one app frame the current appearance. Called as the frame loads, since
 *  a push to a document that has not run the SDK yet reaches nobody. */
export function pushAppearanceToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, APPEARANCE_CHANNEL, paintedAppearance());
}

let pushQueued = false;

function pushToEveryFrame(): void {
  pushQueued = false;
  const appearance = paintedAppearance();
  for (const frame of mountedAppFrames()) {
    postToAppFrame(frame, APPEARANCE_CHANNEL, appearance);
  }
}

/** Repaint every open app frame whenever the shell paints. A preference load
 *  paints several things in one task, and a zoom gesture paints once per input
 *  event, so pushes coalesce to one per task. */
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
