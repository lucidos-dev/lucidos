import { prefillCompose } from '../../store/actions/compose';
import { dismissWelcomeSuggestions } from '../../store/actions/preferences';
import { composeHandlers } from './promptFocus';

/** A clickable starter prompt. `label` is the short chip heading; `prompt` is
 *  the full text dropped into the compose input (not sent — the user reviews it
 *  and hits Send). Mirrors the capability examples in the welcome copy. */
interface Suggestion {
  label: string;
  prompt: string;
}

const SUGGESTIONS: Suggestion[] = [
  {
    label: 'Build an app',
    prompt: 'Build me an app that tracks my reading list.',
  },
  {
    label: 'Set up a reminder',
    prompt: 'Remind me every morning at 8am to review my inbox.',
  },
  {
    label: 'Research something',
    prompt: 'Research e-bike options under €3000 and write up what you find.',
  },
];

export function WelcomeMessage() {
  return (
    <div class="response-content markdown-content welcome-message">
      <h2>Welcome to Lucidos</h2>
      <p class="tagline">If You Can Describe It, It Exists.</p>
      <p>
        I'm the Lucidos Agent — I remember our conversations, keep your files
        and notes as artifacts, and act on your behalf: research, schedules,
        apps, automations.
      </p>
      <div class="welcome-suggestions">
        <p class="welcome-suggestions-label">New here? Tap one to start:</p>
        <div class="welcome-suggestion-chips">
          {SUGGESTIONS.map((s) => (
            <button
              key={s.label}
              type="button"
              class="welcome-suggestion-chip"
              // composeHandlers focuses the prompt within the tap gesture
              // (iOS keyboard) BEFORE prefilling, so a mobile tap lands the
              // text with the keyboard already open.
              {...composeHandlers(() => prefillCompose(s.prompt))}
            >
              <span class="welcome-suggestion-chip-label">{s.label}</span>
              <span class="welcome-suggestion-chip-prompt">“{s.prompt}”</span>
            </button>
          ))}
        </div>
      </div>
      <p>
        Want to change code instead? Point the picker below at a coding
        target — the Lucidos source, an app, or one of your repositories — and
        a coding agent takes the thread, proposing changes you review.
      </p>
      <button
        type="button"
        class="welcome-dismiss"
        onClick={() => { void dismissWelcomeSuggestions(); }}
      >
        Don't show this again
      </button>
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
    </div>
  );
}
