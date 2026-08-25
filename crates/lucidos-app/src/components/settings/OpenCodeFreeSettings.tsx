import { preferences } from '../../store/store';
import {
  currentOpenCodeFreeEnabled,
  setOpenCodeFreeEnabled,
} from '../../store/actions/preferences';
import { Explainer } from '../shared/Explainer';

/** Turn the keyless OpenCode Free tier on or off (Settings → Models →
 *  Providers). There is no key field: the relay serves these models
 *  anonymously, and the engine sends no Authorization header at all. Stored as
 *  the `opencode_free_enabled` preference, which the engine's provider
 *  subscriber watches, so the switch takes effect with no restart.
 *
 *  The privacy line renders beside the toggle rather than behind the Explainer.
 *  Turning this on sends prompts to a third party, so the terms belong where
 *  the decision is made.
 *
 *  Renders only the provider block; the enclosing "Providers"
 *  `settings-section` is owned by `SettingsView`. */
export function OpenCodeFreeSettings() {
  // Subscribe to the preference signal.
  preferences.value;
  const enabled = currentOpenCodeFreeEnabled();

  return (
    <>
      <div class="settings-row" data-search-anchor="providers:opencode-free">
        <span class="settings-row-label">
          OpenCode Free (keyless)
          <Explainer title="OpenCode Free (keyless)">
            <p>
              Free models served anonymously by OpenCode's Zen relay. No account, no
              API key, no billing details. Useful for trying Lucidos, or as a fallback
              when you would rather not spend on a small task.
            </p>
            <p>
              The models appear in the picker as ordinary rows on the{' '}
              <strong>opencode-free</strong> provider. They are smaller than the
              frontier models and the free catalog rotates, so one can stop answering
              without notice.
            </p>
            <p>
              Also settable via the <strong>LUCIDOS_OPENCODE_FREE</strong> launch
              environment variable.
            </p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) =>
              void setOpenCodeFreeEnabled((e.currentTarget as HTMLInputElement).checked)
            }
          />
          <span class="toggle-slider" />
        </label>
      </div>
      <div class="settings-row-note">
        Requests go to a third-party relay with no account and no key. Several of these
        free models may train on what you send them, so keep private work on a provider
        you have configured with your own credential.
      </div>
    </>
  );
}
