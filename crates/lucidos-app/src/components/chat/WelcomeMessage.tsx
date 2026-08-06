import { useSignal } from '@preact/signals';
import { dismissWelcomeSuggestions } from '../../store/actions/preferences';
import { openProviderSettings } from '../../store/actions/menu';
import { applySuggestion, startSetupInterview } from '../../store/actions/compose';
import { composeHandlers } from './promptFocus';
import { llmConfigured } from '../../store/store';
import { ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';

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

/** First-run entry point into the *setup interview*: the Lucidos Agent asks
 *  what the user wants help with, at work or outside it, then builds the apps,
 *  triggers and knowhow that fit, in this session
 *  (`system-knowhow/setup-interview` drives it).
 *
 *  This is NOT a sixth starter suggestion, and it deliberately looks like a
 *  sibling of `ProviderSetupWelcome`'s CTA instead: same prominent
 *  `action-btn action-btn-confirm` plus a hint line. A suggestion hands the
 *  newcomer a sentence and leaves them holding the hard part, which is working
 *  out what Lucidos should do for THEM; this hands that part to the agent.
 *
 *  Clicking SENDS (see `startSetupInterview`) rather than prefilling, so the
 *  interview starts on one gesture. Only rendered on the provider-configured
 *  branch: with no provider the whole surface is `ProviderSetupWelcome`, because
 *  an interview that cannot reach a model is worse than no button.
 *
 *  Dismissing the welcome hides this with it, which is what "Don't show this
 *  again" says. The durable way back is the header's help button
 *  (`SetupInterviewButton`), which is never dismissed. */
export function SetupInterviewWelcome() {
  return (
    <div class="welcome-setup-interview">
      <button
        type="button"
        class="action-btn action-btn-confirm welcome-setup-interview-btn"
        {...composeHandlers(() => { void startSetupInterview(); })}
      >
        Help me get the most out of Lucidos
      </button>
      <p class="welcome-setup-interview-hint">
        A few questions about what you want help with, at work or outside it:
        personal admin, training, learning, whatever fits. Then I build the apps
        and automations to match, right here in your workspace. You can start
        this again any time from the <span aria-hidden="true">?</span> button, or
        by just asking me to set you up.
      </p>
    </div>
  );
}

/** Conversational starter ideas — example things to ask the Lucidos Agent.
 *  Clicking a suggestion drops it into the prompt (via `applySuggestion`, which
 *  targets the Lucidos Agent and confirms before overriding an in-progress draft)
 *  so the user can edit and send, rather than retyping it. Shown one at a time in
 *  a chevron carousel (SuggestionCarousel). */
const IDEAS: string[] = [
  'Build me an app that tracks my reading list.',
  'Send a summary of my emails for review every weekday at 8am.',
  'Research e-bike options under €3000 and write up what you find — then set up a daily web scraper for relevant bargains and notify me when a good one comes up.',
  'Tell me how to set up Lucidos for mobile access.',
  'Where can I download apps?',
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

/** One suggestion at a time, flanked by ‹ › chevrons. Both chevrons stay mounted
 *  on every step — the inapplicable one (at the first / last suggestion) is
 *  `disabled` (greyed, non-interactive) rather than hidden, so the carousel
 *  controls never disappear. Same circular nav treatment as the notification
 *  detail chevrons. The suggestion itself is a button: clicking it drops it into
 *  the prompt (focus-first via `composeHandlers` so the iOS keyboard opens within
 *  the tap gesture). Kept in its own component so WelcomeMessage stays hook-free
 *  (the welcome tests invoke it as a plain function).
 *
 *  All suggestions are rendered stacked in a single CSS grid cell
 *  (`.welcome-carousel-viewport`) so the viewport's height is driven by the
 *  TALLEST suggestion, not the currently-visible one. Suggestions vary in length,
 *  so a viewport sized to the visible card would change height per slide and the
 *  vertically-centered chevrons would bounce with it. Only the current suggestion
 *  is visible and interactive; the rest stay laid out (so they still size the
 *  track) but are hidden from sight, hit-testing, the tab order, and the a11y
 *  tree (CSS owns the visibility — see response.css). */
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
        <ChevronLeftIcon />
      </button>
      <div class="welcome-carousel-viewport">
        {IDEAS.map((idea, i) => {
          const active = i === view.index;
          return (
            <button
              key={idea}
              type="button"
              class="welcome-carousel-item"
              aria-hidden={active ? undefined : 'true'}
              tabIndex={active ? undefined : -1}
              aria-label={`Use this suggestion: ${idea}`}
              {...composeHandlers(() => { void applySuggestion(idea); })}
            >
              {idea}
            </button>
          );
        })}
      </div>
      <button
        type="button"
        class="welcome-carousel-nav"
        aria-label="Next suggestion"
        disabled={!view.hasNext}
        onClick={() => { index.value = view.index + 1; }}
      >
        <ChevronRightIcon />
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
      <SetupInterviewWelcome />
      <p class="welcome-suggestions-label">Or ask me anything, for example:</p>
      <SuggestionCarousel />
      <WelcomeDisclaimer />
    </div>
  );
}
