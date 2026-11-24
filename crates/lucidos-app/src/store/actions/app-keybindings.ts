import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { mountedAppFrames } from '../../utils/appFrame';
import { forwardableBindings } from './keybindings';
import { KEYBINDINGS_CHANNEL } from '../../../../../packages/lucidos-sdk/src/keyboardForward';

export function pushKeybindingsToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, KEYBINDINGS_CHANNEL, { bindings: forwardableBindings() });
}

export function installAppKeybindingsSync(): () => void {
  return effect(() => {
    const bindings = forwardableBindings();
    for (const frame of mountedAppFrames()) {
      postToAppFrame(frame, KEYBINDINGS_CHANNEL, { bindings });
    }
  });
}
