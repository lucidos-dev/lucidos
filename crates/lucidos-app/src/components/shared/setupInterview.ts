import { startSetupInterview } from '../../store/actions/compose';
import { showConfirm } from '../../store/store';

export async function confirmAndStartSetupInterview(): Promise<void> {
  const ok = await showConfirm(
    'I will ask a few questions about what you want help with, at work or outside '
    + 'it, then we build the apps and automations that fit, here in your workspace. '
    + 'Nothing gets built until you say yes to what I propose.'
    + '\n\n'
    + 'Anything else you need help with? Just ask in the chat, I know Lucidos '
    + 'very well.',
    'Start',
    {
      title: 'Need help getting the most out of Lucidos?',
      cancelLabel: 'Not now',
      variant: 'default',
    },
  );
  if (!ok) return;
  await startSetupInterview();
}
