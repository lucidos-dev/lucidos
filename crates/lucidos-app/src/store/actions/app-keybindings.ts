import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { forwardableBindings } from './keybindings';
// The SDK module that listens on the channel. Relative, because the barrel
// does not export it; it has no import-time side effects.
import { KEYBINDINGS_CHANNEL } from '../../../../../packages/lucidos-sdk/src/keyboardForward';

/** Hand one app frame the current bindings. Called as the frame loads, since
 *  a push to a document that has not run the SDK yet reaches nobody. The SDK
 *  cancels the browser default for a chord bound here, so ⌘P in an app opens
 *  file search and not the print dialog. */
export function pushKeybindingsToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, KEYBINDINGS_CHANNEL, { bindings: forwardableBindings() });
}

/** Keep every open app frame's copy current. `forwardableBindings` reads the
 *  preferences signal, so a rebind in Settings re-runs this. Returns the
 *  teardown. */
export function installAppKeybindingsSync(): () => void {
  return effect(() => {
    const bindings = forwardableBindings();
    for (const frame of document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]')) {
      postToAppFrame(frame, KEYBINDINGS_CHANNEL, { bindings });
    }
  });
}
