import { effect } from '@preact/signals';
import { postToAppFrame } from './app-bridge';
import { mountedAppFrames } from '../../utils/appFrame';
import { focusedThreadId } from '../store';
import { FOCUSED_THREAD_CHANNEL } from '../../../../../packages/lucidos-sdk/src/_bridge';

export function installAppFocusedThreadSync(): () => void {
  return effect(() => {
    const threadId = focusedThreadId.value;
    for (const frame of mountedAppFrames()) {
      postToAppFrame(frame, FOCUSED_THREAD_CHANNEL, { threadId });
    }
  });
}
