
import './store/effects';
import { installAppBridge } from './store/actions/app-bridge';
import { installPaneFocusTracking } from './components/layout/paneFocus';

import { installPressCoverWatch } from './components/chat/pressCover';
import { installDeadKeystrokeProbe } from './components/chat/deadKeystrokeProbe';
import { installToastPressProbe } from './components/shared/toastPressProbe';
import { installAppKeybindingsSync } from './store/actions/app-keybindings';
import { installAppExternalLinksSync } from './store/actions/app-external-links';
import { installAppFocusedThreadSync } from './store/actions/app-focused-thread';
import { installAppFrameMessages } from './store/actions/app-frame-messages';
import { installPendingUploadRestore } from './store/actions/pendingUploadRestore';
import { installUnsentMessageRestore } from './store/actions/unsentMessageRestore';
import { installScaleWheel } from './components/shared/scaleWheelListener';
import { installGlobalClick } from './store/globalClick';
import { installSeenTargetWatch } from './store/actions/seen-target';
import { installViewMenu } from './store/actions/view-menu';
import { startPageHealthLog } from './utils/pageHealthLog.report';
import { installResumeClock } from './utils/resumeClock';
import { installShellScrollGuard } from './utils/shellScrollGuard';
import { reportShellSnapBack } from './utils/shellScrollProbe';
import { pushTrafficLightOffset } from './store/actions/trafficLights';
import * as scrollState from './components/chat/scrollState';
import { provideScrollState } from './components/chat/scrollStateHandle';
import * as drawerOrder from './components/drawer/drawerOrder';
import { provideDrawerOrder } from './store/drawerOrderHandle';
import { rememberFocusedViewRows } from './store/actions/threads';
import { watchTranscriptLiveness } from './store/transcriptLiveness';
import { focusedThreadId, threadMap, isThreadStreaming } from './store/store';
import { voiceCall } from './store/voiceCall';
import { isOnCall } from './voice/callState';

export function startShell(): () => void {
  provideScrollState(scrollState);
  provideDrawerOrder(drawerOrder);
  rememberFocusedViewRows();
  // Three diagnostics, not features: the composer's buttons, its textarea and
  // the toasts' buttons. See their headers and docs/temporary-measures.md.
  installPressCoverWatch();
  import('./components/chat/deadPressProbe').then(
    (probe) => probe.installDeadPressProbe(),
    (err: unknown) => console.warn('[shell] could not load the press probe:', err),
  );
  installDeadKeystrokeProbe();
  installToastPressProbe();
  installSeenTargetWatch();
  const stops = [
    installAppBridge(),
    installPaneFocusTracking(),
    installGlobalClick(),
    installAppKeybindingsSync(),
    installAppExternalLinksSync(),
    installAppFocusedThreadSync(),
    installAppFrameMessages(),
    installPendingUploadRestore(),
    installUnsentMessageRestore(),
    installScaleWheel(),
    installViewMenu(),
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
