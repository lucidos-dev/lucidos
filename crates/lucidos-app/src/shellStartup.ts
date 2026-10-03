/** Shell startup: what only a drawn UI can need, installed when the shell
 *  chunk resolves and before `<App/>` first renders.
 *
 *  Nothing here can matter any earlier. Nobody can press, type, open an app
 *  frame or see a restored message before the shell renders. So each install
 *  still runs ahead of the first frame that could need it, and its code stays
 *  out of the entry chunk. See *shell startup* in `docs/glossary.md`. */

import { installDeadPressProbe } from './components/chat/deadPressProbe';
import { installDeadKeystrokeProbe } from './components/chat/deadKeystrokeProbe';
import { installToastPressProbe } from './components/shared/toastPressProbe';
import { installAppKeybindingsSync } from './store/actions/app-keybindings';
import { installAppFrameMessages } from './store/actions/app-frame-messages';
import { installPendingUploadRestore } from './store/actions/pendingUploadRestore';
import { installUnsentMessageRestore } from './store/actions/unsentMessageRestore';

/** Install the shell's document-level listeners. Returns the teardown. */
export function startShell(): () => void {
  // Three diagnostics, not features: the composer's buttons, its textarea and
  // the toasts' buttons. See their headers and docs/temporary-measures.md.
  installDeadPressProbe();
  installDeadKeystrokeProbe();
  installToastPressProbe();
  const stops = [
    installAppKeybindingsSync(),
    installAppFrameMessages(),
    // Images a previous page load was still uploading come back as chips.
    installPendingUploadRestore(),
    // Sends a previous page load never learned the outcome of come back as
    // unsent messages, with Retry.
    installUnsentMessageRestore(),
  ];
  return () => {
    for (const stop of stops) stop();
  };
}
