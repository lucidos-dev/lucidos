import { formatBytes } from '../utils/formatBytes';
import type { AppUpdateRunning } from '../utils/tauri';
import type { ProgressDialogState } from './types';

/** The copy behind every progress dialog, and nothing else.
 *
 *  Pure on purpose, so the three callers share one wording: the store computed
 *  that picks which dialog is live, the packaged updater, and the surface
 *  gallery. The gallery is allowed to import this module precisely because
 *  there is no action in here to reach.
 *
 *  See docs/plans/2026-08-14-the-restart-is-a-dialog-not-a-toast.md. */

/** How the packaged update should read for a given progress frame: the
 *  sentence, the determinate fraction (or `null` when there is no honest one),
 *  and whether the run can still be abandoned.
 *
 *  The SINGLE derivation behind both the dialog and Settings → System. The two
 *  must never disagree about what the update is doing. Terminal frames
 *  (`cancelled` / `failed`) are deliberately absent: they end the run rather
 *  than describe it, so their callers close the dialog instead of updating it. */
export interface AppUpdateNarration {
  message: string;
  /** Determinate fraction in [0, 1], or `null` when there is no honest one.
   *  Clamped HERE rather than at each renderer, since the dialog and the System
   *  page both paint it. A server whose `Content-Length` undercounts the body
   *  would otherwise send one past the end of its own track. */
  progress: number | null;
  cancellable: boolean;
}

/** `Lucidos 2026.7.30`, or a version-less fallback for the window before the
 *  check has resolved one. */
function updateLabel(version: string | null): string {
  return version ? `Lucidos ${version}` : 'the update';
}

export function appUpdateNarration(frame: AppUpdateRunning): AppUpdateNarration {
  const name = updateLabel(frame.version);
  switch (frame.phase) {
    case 'checking':
      // Cancellable: the check is a network round-trip that can hang on a bad
      // connection, and the Rust side races it against the cancel signal too.
      return { message: 'Checking for updates…', progress: null, cancellable: true };
    case 'downloading': {
      // No `Content-Length` means no honest percentage. Show the bytes moving
      // and let the spinner carry the rest. Never a fabricated bar.
      const { downloaded, total } = frame;
      const sized = total !== null && total > 0;
      return {
        message: `Downloading ${name}: ${formatBytes(downloaded)}${sized ? ` of ${formatBytes(total)}` : ''}`,
        progress: sized ? Math.min(1, downloaded / total) : null,
        cancellable: true,
      };
    }
    case 'verifying':
      // Signature check over the buffered bytes, sub-second, and nothing has
      // touched the disk yet. A cancel WOULD still land here, since Rust runs
      // the check inside the abortable download. A button that appears and
      // vanishes within a few hundred ms is noise, so it is withheld. Hiding a
      // working affordance is safe; the rule is only never to OFFER one that
      // cannot work. Full bar: the transfer really is complete.
      return { message: `Verifying ${name}…`, progress: 1, cancellable: false };
    case 'installing':
      return { message: `Installing ${name}…`, progress: null, cancellable: false };
    case 'restarting-services':
      return { message: 'Restarting background services…', progress: null, cancellable: false };
    case 'relaunching':
      return { message: 'Relaunching Lucidos…', progress: null, cancellable: false };
  }
}

/** What every restart says while the engine is away. One sentence, because the
 *  title already names the operation and the spinner already says it is going. */
const RESTART_DIALOG_MESSAGE = 'The workspace is unavailable until the engine comes back.';

/** The engine restart, in the two shapes it takes.
 *
 *  `newVersion` is the same predicate that drives the switch badge. So the
 *  dialog claims a new version only when the running binary and the one we
 *  respawn onto differ. A plain restart says so instead. No lies.
 *
 *  No progress: a respawn has no honest percentage. No Cancel either, because a
 *  restart cannot be called back once the engine is going down. */
export function restartDialogState(newVersion: boolean): ProgressDialogState {
  return {
    visible: true,
    title: newVersion ? 'Starting new version' : 'Restarting engine',
    message: RESTART_DIALOG_MESSAGE,
    progress: null,
  };
}

/** One frame of the packaged install, as a dialog.
 *
 *  `onCancel` is passed in rather than imported, which is what keeps this module
 *  free of the updater. It is offered only while the narration says abandoning
 *  the run can still work. */
export function appUpdateDialogState(
  frame: AppUpdateRunning,
  onCancel: () => void,
): ProgressDialogState {
  const narration = appUpdateNarration(frame);
  return {
    visible: true,
    title: 'Updating Lucidos',
    message: narration.message,
    progress: narration.progress,
    cancel: narration.cancellable ? { label: 'Cancel', onClick: onCancel } : undefined,
  };
}
