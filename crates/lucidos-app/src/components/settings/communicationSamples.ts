import {
  showToast,
  showConfirm,
  showPrompt,
  dismissToast,
  progressDialog,
  TOAST_AUTO_DISMISS_MS,
  type ToastPlacement,
} from '../../store/store';
import type { DropdownOption } from '../shared/Dropdown';

/** Everything the communication-surface gallery fires, kept out of the page so
 *  the page is layout and this is content.
 *
 *  Every sample here is INERT. Nothing restarts, downloads, navigates, or
 *  writes a preference. That is the whole point of the gallery: the surfaces
 *  can be judged before a real flow is pointed at any of them. A source scan
 *  guards it (`__tests__/communication-samples-inert.test.ts`).
 *
 *  Copy is taken from the real call sites rather than invented, so a surface is
 *  judged at the lengths that actually occur. */

// --- Toast placement (temporary, see docs/temporary-measures.md) ---

/** Labels say what the shape DOES, not what it is called internally. The picker
 *  exists to answer "which of these looks right", and someone choosing between
 *  them has no other description to go on.
 *
 *  Typing `value` as the union is what keeps this list and the union in step: a
 *  typo here fails the build instead of offering an unpickable shape. */
export const TOAST_PLACEMENT_OPTIONS: (DropdownOption & { value: ToastPlacement })[] = [
  { value: 'bottom-right', label: 'Bottom-right corner', description: 'Desktop only, anchored to the window. Mobile keeps the top' },
  { value: 'top-bleed', label: 'Full width, top', description: 'One bar across both panes, under the header' },
  { value: 'bottom-bleed', label: 'Full width, bottom', description: 'One bar across both panes, over the composer' },
  { value: 'card', label: 'Centred card', description: 'One card centred on the window, crossing the divider' },
  { value: 'pane', label: 'Per pane', description: 'One column centred over each pane' },
];

// --- Toasts ---

/** Keyed for two reasons. A second press then REPLACES the sample rather than
 *  piling up duplicates, and a toast's own buttons can dismiss it by name. A
 *  sample with a dead button would misrepresent the shape it is there to show. */
const SAMPLE_SHORT = 'sample-toast-short';
const SAMPLE_LONG = 'sample-toast-long';
const SAMPLE_ACTIONS = 'sample-toast-actions';
const SAMPLE_ERROR = 'sample-toast-error';
const SAMPLE_PROGRESS = 'sample-toast-progress';

export function sampleShortToast(): void {
  showToast('Trigger saved.', 'success', { key: SAMPLE_SHORT, autoDismissMs: TOAST_AUTO_DISMISS_MS });
}

export function sampleLongToast(): void {
  showToast(
    'Merge conflict in "Put toasts where they originate" resolved automatically. '
    + 'The change applied on top of the current main and is ready to review.',
    'warning',
    { key: SAMPLE_LONG },
  );
}

export function sampleErrorToast(): void {
  showToast('Failed to apply change: the worktree has uncommitted edits.', 'error', { key: SAMPLE_ERROR });
}

export function sampleActionToast(): void {
  showToast('New engine version available.', 'info', {
    key: SAMPLE_ACTIONS,
    action: { label: 'Restart', onClick: () => dismissToast(SAMPLE_ACTIONS) },
    secondaryAction: { label: 'Later', onClick: () => dismissToast(SAMPLE_ACTIONS) },
  });
}

export function sampleProgressToast(): void {
  showToast('Downloading embedding model', 'info', {
    key: SAMPLE_PROGRESS,
    spinning: true,
    progress: 0.42,
    secondaryAction: { label: 'Cancel', onClick: () => dismissToast(SAMPLE_PROGRESS) },
  });
}

/** All five at once, staggered. A shape has to hold up as a STACK as well as
 *  one at a time, and toasts do arrive in bursts. */
export function sampleToastBurst(): void {
  sampleShortToast();
  setTimeout(sampleLongToast, 300);
  setTimeout(sampleErrorToast, 600);
  setTimeout(sampleActionToast, 900);
}

// --- Dialogs ---

export function sampleConfirmDanger(): void {
  void showConfirm('Delete the trigger "Daily digest"? This cannot be undone.', 'Delete', {
    title: 'Delete trigger',
  });
}

export function sampleConfirmDefault(): void {
  void showConfirm('Restart the engine to activate the applied changes?', 'Restart', {
    title: 'Restart engine',
    variant: 'default',
  });
}

export function samplePrompt(): void {
  void showPrompt('What should this thread be called?', {
    title: 'Rename thread',
    defaultValue: 'Put toasts where they originate',
  });
}

/** The three acknowledgements: something has already happened and the user has
 *  to see it. Nothing to decide, so each carries a lone [OK] and no Cancel. */
export function sampleAcknowledgeWedged(): void {
  void showConfirm(
    'New engine version pending, and rebuilding cannot deliver it: a build for '
    + 'this commit already succeeded without producing one.\n\n'
    + 'Relaunch the stack from your checkout.',
    'OK',
    { title: 'Cannot deliver the new version', acknowledge: true, variant: 'default' },
  );
}

export function sampleAcknowledgeDeferred(): void {
  void showConfirm(
    "Frontend change applied. It'll take effect when you switch to the new version.",
    'OK',
    { title: 'Frontend change applied', acknowledge: true, variant: 'default' },
  );
}

export function sampleAcknowledgeStranded(): void {
  void showConfirm(
    'Frontend change applied but not served yet: the build watch has not rebuilt.\n\n'
    + 'It will appear on its own once it does.',
    'OK',
    { title: 'Not served yet', acknowledge: true, variant: 'default' },
  );
}

/** The cross-thread consent prompt: another thread asks to put something on
 *  screen, and the user can decline. */
export function sampleConsentPrompt(): void {
  void showConfirm(
    '"Nightly digest" wants to open the Files panel.',
    'Open',
    { title: 'A thread wants to navigate', cancelLabel: 'Not now', variant: 'default' },
  );
}

// --- Progress dialog ---

/** Phases the packaged install really walks, with the fractions it reports.
 *  A null fraction is a phase with no honest percentage. */
const INSTALL_PHASES: { message: string; progress: number | null; cancellable: boolean }[] = [
  { message: 'Checking for a new version…', progress: null, cancellable: true },
  { message: 'Downloading Lucidos 0.31.0…', progress: 0.18, cancellable: true },
  { message: 'Downloading Lucidos 0.31.0…', progress: 0.64, cancellable: true },
  { message: 'Verifying the download…', progress: 0.92, cancellable: true },
  { message: 'Installing…', progress: null, cancellable: false },
  { message: 'Relaunching Lucidos…', progress: null, cancellable: false },
];

let ticker: ReturnType<typeof setInterval> | null = null;

/** Stop the fake run and clear the slot. Idempotent, so the page can call it on
 *  unmount without knowing whether a run is going. */
export function stopSampleProgressDialog(): void {
  if (ticker !== null) {
    clearInterval(ticker);
    ticker = null;
  }
  progressDialog.value = { visible: false, title: '', message: '', progress: null };
}

/** Walk the install phases on a slow ticker, so every one can be looked at.
 *  The real run passes through some of these in under a second, which is
 *  exactly why the gallery does not reuse its timing. */
export function playSampleProgressDialog(): void {
  stopSampleProgressDialog();
  let i = 0;
  const render = () => {
    const phase = INSTALL_PHASES[i];
    progressDialog.value = {
      visible: true,
      title: 'Updating Lucidos',
      message: phase.message,
      progress: phase.progress,
      cancel: phase.cancellable
        ? { label: 'Cancel', onClick: stopSampleProgressDialog }
        : undefined,
    };
  };
  render();
  ticker = setInterval(() => {
    i += 1;
    if (i >= INSTALL_PHASES.length) {
      stopSampleProgressDialog();
      return;
    }
    render();
  }, 2000);
}

/** Hold one phase still, for looking at the layout rather than the sequence. */
export function showSampleProgressPhase(index: number): void {
  stopSampleProgressDialog();
  const phase = INSTALL_PHASES[Math.min(Math.max(index, 0), INSTALL_PHASES.length - 1)];
  progressDialog.value = {
    visible: true,
    title: 'Updating Lucidos',
    message: phase.message,
    progress: phase.progress,
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}

/** The engine restart, which shares the dialog and has no honest percentage. */
export function sampleRestartDialog(): void {
  stopSampleProgressDialog();
  progressDialog.value = {
    visible: true,
    title: 'Starting new version',
    message: 'The workspace is unavailable until the engine comes back.',
    progress: null,
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}
