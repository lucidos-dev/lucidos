import { startSetupInterview } from '../../store/actions/compose';
import { showConfirm } from '../../store/store';

/** Confirm, then start the *setup interview*.
 *
 *  Shared by the desktop header's action and the Lucidos menu's row, so the
 *  confirm copy has one home. It is long, load-bearing prose (it sets
 *  expectations about what the interview will build and promises nothing is
 *  built unsupervised), and two hand-kept copies of it would drift.
 *
 *  **It confirms before it fires**, unlike the welcome CTA. The difference is
 *  the surface, not the action: the welcome CTA is a large deliberate button on
 *  an otherwise empty screen, while these entry points are small and sit beside
 *  other controls, and they SEND a message rather than opening a view. A mis-tap
 *  next to New thread should not post an interview request into the thread.
 *
 *  The header BUTTON that used to live here is gone: the desktop row's controls
 *  are data now (`threadHeaderActions`), so they can fold into the ⋯ overflow
 *  menu when the pane narrows, and a component could not. */
export async function confirmAndStartSetupInterview(): Promise<void> {
  const ok = await showConfirm(
    'I will ask a few questions about what you want help with, at work or outside '
    + 'it, then build the apps and automations that fit, here in your workspace. '
    + 'Nothing gets built until you say yes to what I propose.'
    // Blank line: `DialogMessage` renders this as its own paragraph.
    + '\n\n'
    + 'Anything else you need help with? Just ask in the chat, I know Lucidos '
    + 'very well.',
    'Start',
    {
      title: 'Need help getting the most out of Lucidos?',
      cancelLabel: 'Not now',
      // Starting the interview is an invitation, not a destructive act, so the
      // CTA takes the shared action blue rather than the confirm dialog's
      // danger-red default.
      variant: 'default',
    },
  );
  if (!ok) return;
  await startSetupInterview();
}
