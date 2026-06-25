import { useSignal } from '@preact/signals';
import { dismissWelcomeSuggestions } from '../../store/actions/preferences';
import { openProviderSettings } from '../../store/actions/menu';
import { llmConfigured } from '../../store/store';
import { ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';

/** The MIT-license disclaimer shown at the foot of every welcome variant. */
function WelcomeDisclaimer() {
  return (
    <p class="disclaimer">
      Provided as is under the{' '}
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
 *  so this replaces the starter suggestions with a single clear call to action
 *  that deep-links to Settings → Models → Providers. Shown regardless of the
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

/** Conversational starter ideas — example things to ask the Lucidos Agent.
 *  These are illustrative copy, not interactive: the user reads them and types
 *  their own request. Shown one at a time in a chevron carousel (SuggestionCarousel). */
const IDEAS: string[] = [
  'Build me an app that tracks my reading list.',
  'Send a summary of my emails for review every weekday at 8am.',
  'Research e-bike options under €3000 and write up what you find — then set up a daily web scraper for relevant bargains and notify me when a good one comes up.',
  'Tell me how to set up Lucidos for mobile access.',
  'Show me the app store.',
];

/** Pure view-model for {@link SuggestionCarousel}: clamps `index` into range and
 *  reports the current suggestion plus whether the prev/next chevrons apply. */
export function suggestionView(ideas: string[], index: number) {
  const last = ideas.length - 1;
  const clamped = Math.max(0, Math.min(index, last));
  return {
    current: ideas[clamped],
    index: clamped,
    hasPrev: clamped > 0,
    hasNext: clamped < last,
  };
}

/** One suggestion at a time, flanked by ‹ › chevrons. The chevrons are disabled
 *  at the ends ("when applicable"). Kept in its own component so WelcomeMessage
 *  stays hook-free (the welcome tests invoke it as a plain function). */
export function SuggestionCarousel() {
  const index = useSignal(0);
  const view = suggestionView(IDEAS, index.value);
  return (
    <div class="welcome-carousel">
      <button
        type="button"
        class="welcome-carousel-nav"
        aria-label="Previous suggestion"
        disabled={!view.hasPrev}
        onClick={() => { index.value = view.index - 1; }}
      >
        <ChevronLeftIcon size="1rem" />
      </button>
      <p class="welcome-carousel-item">{view.current}</p>
      <button
        type="button"
        class="welcome-carousel-nav"
        aria-label="Next suggestion"
        disabled={!view.hasNext}
        onClick={() => { index.value = view.index + 1; }}
      >
        <ChevronRightIcon size="1rem" />
      </button>
    </div>
  );
}

export function WelcomeMessage() {
  // No provider configured → guide the user to set one up instead of offering
  // starter prompts that would chat into a guaranteed "no provider" error.
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
      <p class="welcome-suggestions-label">A few suggestions to get you started:</p>
      <SuggestionCarousel />
      <WelcomeDisclaimer />
    </div>
  );
}
