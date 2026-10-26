/** Shell startup: what only a drawn UI can need, installed when the shell
 *  chunk resolves and before `<App/>` first renders.
 *
 *  Nothing here can matter any earlier. Nobody can press, type, open an app
 *  frame or see a restored message before the shell renders. So each install
 *  still runs ahead of the first frame that could need it, and its code stays
 *  out of the entry chunk. See *shell startup* in `docs/glossary.md`.
 *
 *  One install loads on its own: the composer's press probe. It is the largest
 *  module here and only a diagnostic, so it arms one chunk fetch after the
 *  shell instead of growing the shell chunk. Its pointerdown cover sample must
 *  run before any overlay's listener, so that part installs here at once. */

import { installPressCoverWatch } from './components/chat/pressCover';
import { installDeadKeystrokeProbe } from './components/chat/deadKeystrokeProbe';
import { installToastPressProbe } from './components/shared/toastPressProbe';
import { installAppKeybindingsSync } from './store/actions/app-keybindings';
import { installAppFocusedThreadSync } from './store/actions/app-focused-thread';
import { installAppFrameMessages } from './store/actions/app-frame-messages';
import { installPendingUploadRestore } from './store/actions/pendingUploadRestore';
import { installUnsentMessageRestore } from './store/actions/unsentMessageRestore';
import { installScaleWheel } from './components/shared/scaleWheelListener';
import { installSeenTargetWatch } from './store/actions/seen-target';
import { startPageHealthLog } from './utils/pageHealthLog.report';
import { installResumeClock } from './utils/resumeClock';
import { installShellScrollGuard } from './utils/shellScrollGuard';
import { reportShellSnapBack } from './utils/shellScrollProbe';
import { pushTrafficLightOffset } from './store/actions/trafficLights';
import * as scrollState from './components/chat/scrollState';
import { provideScrollState } from './components/chat/scrollStateHandle';
import { watchTranscriptLiveness } from './store/transcriptLiveness';
import { focusedThreadId, threadMap, isThreadStreaming } from './store/store';
import { voiceCall } from './store/voiceCall';
import { isOnCall } from './voice/callState';

/** Install the shell's document-level listeners. Returns the teardown. */
export function startShell(): () => void {
  // FIRST, so the store's scroll calls queued during boot replay before
  // anything else here runs. See components/chat/scrollStateHandle.ts.
  provideScrollState(scrollState);
  // Three diagnostics, not features: the composer's buttons, its textarea and
  // the toasts' buttons. See their headers and docs/temporary-measures.md.
  installPressCoverWatch();
  // A failed load is telemetry, not a toast: nobody asked for the probe, and
  // the next page load arms it again.
  import('./components/chat/deadPressProbe').then(
    (probe) => probe.installDeadPressProbe(),
    (err: unknown) => console.warn('[shell] could not load the press probe:', err),
  );
  installDeadKeystrokeProbe();
  installToastPressProbe();
  // The *seen target* rule: a notification clears once the reader has looked
  // at what its tap points at. Nothing is on screen to look at before this.
  // See actions/seen-target.ts and system-knowhow/notifications.md §4.
  installSeenTargetWatch();
  const stops = [
    installAppKeybindingsSync(),
    installAppFocusedThreadSync(),
    installAppFrameMessages(),
    // Images a previous page load was still uploading come back as chips.
    installPendingUploadRestore(),
    // Sends a previous page load never learned the outcome of come back as
    // unsent messages, with Retry.
    installUnsentMessageRestore(),
    // Cmd/Ctrl + wheel steps the UI scale, as Cmd +/- does.
    installScaleWheel(),
    // A diagnostic, not a feature: docs/temporary-measures.md § Page health log.
    startPageHealthLog(),
    // A diagnostic, not a feature: docs/temporary-measures.md § App frame load probe.
    installResumeClock(),
    // The desktop shell never scrolls as a whole. A snap-back moves the header
    // band, so the traffic lights re-centre on it. The report is a diagnostic:
    // docs/temporary-measures.md § Shell snap-back log.
    installShellScrollGuard((snap) => {
      pushTrafficLightOffset();
      reportShellSnapBack(snap);
    }),
    // The transcript's follow parks on a scroll unless the thread on screen is
    // live streaming: the agent running, or a call up on it. A call writes no
    // turn, so the projection alone reads it as quiet. See transcriptLiveness.ts.
    watchTranscriptLiveness({
      focused: focusedThreadId,
      threads: threadMap,
      isLive: (thread) => isThreadStreaming(thread)
        || (isOnCall(voiceCall.value.phase) && voiceCall.value.threadId === thread.meta.id),
      setLive: scrollState.setTranscriptLive,
    }),
  ];
  return () => {
    for (const stop of stops) stop();
  };
}
