import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { forwardableBindings } from './keybindings';

/** Push channel carrying the shortcut bindings to app frames. Must match
 *  `KEYBINDINGS_CHANNEL` in `packages/lucidos-sdk/src/keyboardForward.ts`. The
 *  SDK cancels the browser default for a chord bound here, so ⌘P in an app
 *  opens file search and not the print dialog. */
export const APP_KEYBINDINGS_CHANNEL = 'keybindings';

/** Hand one app frame the current bindings. Called as the frame loads, since
 *  a push to a document that has not run the SDK yet reaches nobody. */
export function pushKeybindingsToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, APP_KEYBINDINGS_CHANNEL, { bindings: forwardableBindings() });
}

/** Keep every open app frame's copy current. `forwardableBindings` reads the
 *  preferences signal, so a rebind in Settings re-runs this. */
export function installAppKeybindingsSync(): void {
  effect(() => {
    const bindings = forwardableBindings();
    for (const frame of document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]')) {
      postToAppFrame(frame, APP_KEYBINDINGS_CHANNEL, { bindings });
    }
  });
}
