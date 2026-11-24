import { preferences } from '../../store/store';
import {
  currentOpenCodeFreeEnabled,
  setOpenCodeFreeEnabled,
} from '../../store/actions/preferences';
import { Explainer } from '../shared/Explainer';
import { providerAnchor } from '../../store/modelProviders';

export function OpenCodeFreeSettings() {
  preferences.value;
  const enabled = currentOpenCodeFreeEnabled();

  return (
    <>
      <div class="settings-row" data-search-anchor={providerAnchor('opencode-free')}>
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
