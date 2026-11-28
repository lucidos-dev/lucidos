import { dismissWelcomeSuggestions } from '../../store/actions/preferences';
import { openFreeProviderSettings, openProviderSettings } from '../../store/actions/menu';
import { startSetupInterview } from '../../store/actions/compose';
import { composeHandlers } from './composeHandlers';
import { llmConfigured } from '../../store/store';
import { viewportIsMobile } from '../../utils/viewport';

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

export function ProviderSetupWelcome() {
  return (
    <div class="response-content markdown-content welcome-message">
      <h2>Welcome to Lucidos</h2>
      <p>
        I'm the Lucidos Agent, but I can't answer yet. This workspace has no AI
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
          Opens Settings → Providers. Add an OpenAI, Anthropic or
          OpenRouter key, or point Lucidos at a local model. It takes effect
          right away, with no restart.
        </p>
        <button
          type="button"
          class="action-btn welcome-provider-free-btn"
          onClick={openFreeProviderSettings}
        >
          No key? Start on the free tier
        </button>
        <p class="welcome-provider-setup-hint">
          OpenCode Free serves smaller models with no account, no key and no
          card. It stays off until you switch it on, and the switch states its
          terms.
        </p>
      </div>
      <WelcomeDisclaimer />
    </div>
  );
}

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
