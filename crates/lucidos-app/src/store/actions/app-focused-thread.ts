import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { mountedAppFrames } from '../../utils/appFrame';
import { focusedThreadId } from '../store';
import { FOCUSED_THREAD_CHANNEL } from '../../../../../packages/lucidos-sdk/src/_bridge';

/** Tell every open app frame when the user opens another thread, for
 *  `lucidos.ui.onFocusedThreadChange`. Widget windows float over the thread
 *  pane, so they hear it too. A frame's first read is the bridge's
 *  `ui.focused-thread` op, never a push. Returns the teardown. */
export function installAppFocusedThreadSync(): () => void {
  return effect(() => {
    const threadId = focusedThreadId.value;
    for (const frame of mountedAppFrames()) {
      postToAppFrame(frame, FOCUSED_THREAD_CHANNEL, { threadId });
    }
  });
}
