import { dismissWelcomeSuggestions } from '../../store/actions/preferences';
import { openProviderSettings } from '../../store/actions/menu';
import { startSetupInterview } from '../../store/actions/compose';
import { composeHandlers } from './promptFocus';
import { llmConfigured } from '../../store/store';
import { viewportIsMobile } from '../../utils/viewport';

/** The MIT-license disclaimer shown at the foot of every welcome variant. */
function WelcomeDisclaimer() {
  return (
    <p class="disclaimer">
      This software is provided as is under the{' '}
      <a
        href="https://github.com/lucidos-dev/lucidos/blob/main/LICENSE"
        target="_blank"
        rel="noopener noreferrer"
      >
        MIT license
      </a>
      . No warranty, and no liability for actions performed or their
      consequences.
    </p>
  );
}

/** First-run state when the engine booted with no LLM provider configured
 *  (`llmConfigured === false`). The agent can't answer until a provider exists,
 *  so this replaces the setup-interview entry point with a single clear call to
 *  action that deep-links to Settings → Models → Providers. Shown regardless of the
 *  "Don't show this again" dismissal — provider setup is a requirement, not a tip. */
export function ProviderSetupWelcome() {
  return (
    <div class="response-content markdown-content welcome-message">
      <h2>Welcome to Lucidos</h2>
      <p>
        I'm the Lucidos Agent — but I can't answer yet. This workspace has no AI
        provider configured. Connect one and I'll be ready to research, schedule,
        build apps, and act on your behalf.
      </p>
      <div class="welcome-provider-setup">
        <button
          type="button"
          class="action-btn action-btn-confirm welcome-provider-setup-btn"
          onClick={openProviderSettings}
        >
          Set up your AI provider
        </button>
        <p class="welcome-provider-setup-hint">
          Opens Settings → Models → Providers. Add an OpenAI / Anthropic /
          OpenRouter key or a local model — it takes effect right away, no
          restart needed.
        </p>
      </div>
      <WelcomeDisclaimer />
    </div>
  );
}

/** First-run entry point into the *setup interview*: the Lucidos Agent asks
 *  what the user wants help with, at work or outside it, then builds the apps,
 *  triggers and knowhow that fit, in this session
 *  (`system-knowhow/setup-interview` drives it).
 *
 *  It is the ONLY action on the configured-workspace welcome, and it has the
 *  same shape as `ProviderSetupWelcome`'s CTA: a prominent `action-btn` plus a
 *  hint line. It stays on the BLUE default rather than taking the green
 *  `action-btn-confirm` variant the provider CTA wears, because green reads as
 *  "accept what is already on screen" and this one starts something. The surface
 *  used to offer starter suggestions beside it, which handed the newcomer a
 *  sentence and left them holding the hard part, which is working out what
 *  Lucidos should do for THEM; this hands that part to the agent, so the
 *  suggestions were dropped rather than kept as an "or".
 *
 *  Clicking SENDS (see `startSetupInterview`) rather than prefilling, so the
 *  interview starts on one gesture. Only rendered on the provider-configured
 *  branch: with no provider the whole surface is `ProviderSetupWelcome`, because
 *  an interview that cannot reach a model is worse than no button.
 *
 *  Dismissing the welcome hides this with it, which is what "Don't show this
 *  again" says, so the hint has to name a durable way back while it can still be
 *  read. That way back differs by viewport, and the hint says whichever is true
 *  here: desktop keeps the header's help button (`SetupInterviewButton`), which
 *  is never dismissed, while mobile has no such button and reaches the interview
 *  by asking. Keyed off `viewportIsMobile`, the same signal that decides which
 *  header is mounted, so the copy cannot drift from the affordance. */
export function SetupInterviewWelcome() {
  const onMobile = viewportIsMobile.value;
  return (
    <div class="welcome-setup-interview">
      <button
        type="button"
        class="action-btn welcome-setup-interview-btn"
        {...composeHandlers(() => { void startSetupInterview(); })}
      >
        Help me get the most out of Lucidos
      </button>
      <p class="welcome-setup-interview-hint">
        A few questions about what you want help with, at work or outside it:
        personal admin, training, learning, whatever fits. Then we build the
        apps and automations to match, right here in your workspace. You can
        start this again any time
        {onMobile ? ' ' : <> from the <span aria-hidden="true">?</span> button, or </>}
        by just asking me to set you up.
      </p>
    </div>
  );
}

export function WelcomeMessage() {
  // No provider configured → guide the user to set one up instead of offering an
  // interview that would chat into a guaranteed "no provider" error.
  if (!llmConfigured.value) {
    return <ProviderSetupWelcome />;
  }
  return (
    <div class="response-content markdown-content welcome-message welcome-hero">
      <button
        type="button"
        class="welcome-dismiss"
        onClick={() => { void dismissWelcomeSuggestions(); }}
      >
        Don't show this again
      </button>
      <h2>Hi, there!</h2>
      <SetupInterviewWelcome />
      <WelcomeDisclaimer />
    </div>
  );
}
