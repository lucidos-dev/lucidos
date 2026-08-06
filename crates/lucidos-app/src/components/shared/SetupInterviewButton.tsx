import { HelpIcon } from './icons';
import { composeHandlers } from '../chat/promptFocus';
import { startSetupInterview } from '../../store/actions/compose';
import { llmConfigured, showConfirm } from '../../store/store';

/** The durable way into the *setup interview*, sitting immediately left of the
 *  New thread (compose) button.
 *
 *  Rendered on the desktop header and the mobile Threads header, and
 *  deliberately NOT on the mobile conversation header: that row centers the
 *  brand absolutely and shrink-to-content, so a fourth trailing icon there
 *  pushes the brand into the icon cluster at 375px (measured by
 *  `e2e/mobile-threads-title-alignment.spec.ts`, which fails on the overlap).
 *  Mobile reaches the interview from the Threads header instead, which is one
 *  swipe away and is where starting something new already lives.
 *
 *  The prominent entry point is the welcome CTA (`SetupInterviewWelcome`), but
 *  that lives inside the dismissible welcome, so it is gone for anyone who hit
 *  "Don't show this again" and for anyone who wants the interview months later.
 *  This button is the answer to both, and it is deliberately NOT gated on the
 *  dismissal: making the welcome CTA outlive its own dismissal would be the dark
 *  pattern, whereas a permanent, quiet header affordance is just a control.
 *
 *  **It confirms before it fires**, unlike the welcome CTA. The difference is
 *  the surface, not the action: the welcome CTA is a large deliberate button on
 *  an otherwise empty screen, while this one is small, permanent and wedged
 *  between other header icons, and it SENDS a message rather than opening a
 *  view. A mis-tap next to New thread should not post an interview request into
 *  the user's thread.
 *
 *  Hidden with no LLM provider configured, matching the welcome's precedence:
 *  the whole first-run surface becomes `ProviderSetupWelcome` in that state
 *  because the agent cannot answer at all, and an interview needs it most. */
export function SetupInterviewButton({ showTooltip }: { showTooltip?: boolean }) {
  if (!llmConfigured.value) return null;
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      data-role="setup-interview-toggle"
      {...composeHandlers(
        () => {
          void (async () => {
            const ok = await showConfirm(
              'I will ask a few questions about what you want help with, at work or outside '
              + 'it, then build the apps and automations that fit, here in your workspace. '
              + 'Nothing gets built until you say yes to what I propose.'
              // Blank line: `DialogMessage` renders this as its own paragraph.
              + '\n\n'
              + 'Anything else you need help with? Just ask in the chat, I know Lucidos '
              + 'very well.',
              'Start',
              { title: 'Need help getting the most out of Lucidos?', cancelLabel: 'Not now' },
            );
            if (!ok) return;
            await startSetupInterview();
          })();
        },
        // No focus nudge. `composeHandlers` defaults to focusing the prompt
        // textarea, which is right for buttons that leave the user typing but
        // wrong here: the next thing on screen is a confirm modal, so on iOS
        // that raises the keyboard behind it and leaves it up if they cancel.
        // The touch/click dedup is what this wrapper is still here for.
        () => {},
      )}
      aria-label="Get the most out of Lucidos"
      data-tooltip={showTooltip ? 'Get the most out of Lucidos: a few questions, then I build what fits' : undefined}
    >
      <HelpIcon />
    </button>
  );
}
