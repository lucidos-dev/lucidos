import { useState } from 'preact/hooks';
import { credentials, preferences } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import { setTypeSafeEnabled } from '../../store/actions/preferences';
import { findProviderCredential } from './providerCredential';
import { ProviderBlockFrame } from './ProviderBlockFrame';
import {
  switchAction,
  typeSafeBlockLoaded,
  typeSafeProviderState,
} from './providerEnablement';
import {
  jevConsumers,
  typeSafeSwitchedOff,
  JUDGMENT_SITE_LOCATION,
  TYPESAFE_API_KEY_ENV,
  TYPESAFE_CREDENTIAL_SERVICE,
} from './judgmentBackend';

/** The API root the engine's judgment provider posts to. */
const TYPESAFE_BASE_URL = 'https://api.typesafe.ai/v1';

/**
 * TypeSafe (Jev), on Settings → Models → Providers.
 *
 * **Still not a `ProviderBlock`.** That component takes a `SwitchableProvider`
 * and pairs a `provider_enabled_*` preference with
 * `/health.configured_providers`. Jev is in neither: it answers typed questions
 * rather than holding a conversation, so it has no `ProviderKind` (ADR 0220).
 *
 * What it does share is the row itself. Both wear `ProviderBlockFrame`, so the
 * switch, the Remove beside it and the folded config rows behave here exactly
 * as they do for xAI. Only the decision differs, and it differs because the
 * evidence does: a stored credential stands in for the `/health` list, and a
 * preference write moves the derived state at once where a `/health` probe
 * lags. See `typeSafeProviderState` for the substitution and its blind spot.
 *
 * Each site picks Jev in its own model control, beside the feature it changes.
 * This is the master switch above both.
 */
export function TypeSafeJudgmentSettings() {
  const credLoadable = credentials.value;
  const prefsLoadable = preferences.value;
  const existing = findProviderCredential(credLoadable, TYPESAFE_CREDENTIAL_SERVICE);

  const loaded = typeSafeBlockLoaded(
    credLoadable.status === 'loaded',
    prefsLoadable.status === 'loaded',
  );
  const state = typeSafeProviderState({
    keyStored: !!existing,
    switchedOff: typeSafeSwitchedOff(prefsLoadable),
  });
  // The local disclosure that lets a first key be typed. A provider nobody has
  // configured has nothing to switch, so pressing its toggle writes nothing and
  // only reveals the fields. Consulted in that state alone, so a switch-off
  // made on another device still closes this block when its preference lands.
  const [expanded, setExpanded] = useState(false);
  const open = loaded && (state === 'on' || (state === 'not-set-up' && expanded));

  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  function onToggle(next: boolean): void {
    const action = switchAction(state, next);
    if (action === 'enable') void setTypeSafeEnabled(true);
    if (action === 'disable') void setTypeSafeEnabled(false);
    if (action === 'expand') setExpanded(true);
    if (action === 'collapse') setExpanded(false);
  }

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(
        TYPESAFE_CREDENTIAL_SERVICE,
        [TYPESAFE_BASE_URL],
        'api_key',
        secret.trim(),
      );
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  const inUseBy = jevConsumers(prefsLoadable, !!existing);

  return (
    <ProviderBlockFrame
      label="TypeSafe (Jev)"
      anchor="models:typesafe"
      explainer={
        <>
          <p>
            Jev answers <strong>typed questions</strong> rather than writing text, so it
            cannot hold a conversation and stays out of the chat model picker. It is
            offered as a model in the two classifications it can answer.
          </p>
          <p>
            Neither moves on its own. Choose <strong>TypeSafe (Jev)</strong> as the model
            in <strong>{JUDGMENT_SITE_LOCATION['command-guard']}</strong> or{' '}
            <strong>{JUDGMENT_SITE_LOCATION['query-classification']}</strong>. This
            switch is above both: off, every classification runs on its chat model.
          </p>
          <p>
            A stored key does give the agent a <strong>judge</strong> tool, so it can ask
            Jev a typed question itself when that is the cheaper way to sort, rank or
            filter a lot at once. The switch above turns that off too.
          </p>
          <p>
            Stored here, the key is used instead of the{' '}
            <strong>{TYPESAFE_API_KEY_ENV}</strong> launch environment variable, which
            stays as a fallback. Apps reach the same key through{' '}
            <strong>lucidos.proxy('typesafe')</strong>, so an app never holds it.
          </p>
        </>
      }
      detail={existing ? <span class="list-row-details">configured</span> : null}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, TYPESAFE_CREDENTIAL_SERVICE)}
        >
          Remove
        </button>
      )}
      switchedOff={state === 'switched-off'}
      hasStoredConfig={!!existing}
      loaded={loaded}
      open={open}
      onToggle={onToggle}
    >
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace secret' : 'Secret'}</span>
        <input
          type="password"
          class="settings-text-input"
          placeholder="ts-…"
          value={secret}
          onInput={(e) => setSecret((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={saving || !secret.trim()}
          onClick={() => void save()}
        >
          {existing ? 'Update' : 'Save'}
        </button>
      </div>
      {/* Three surfaces carry this feature, so the one holding the key says
          what the other two are doing. Without it, pasting a key looks like it
          did nothing.

          Two things keep it honest, and both come from living inside the fold.
          It never names a running site while the provider is off, though the
          preferences it reads outlive the switch. And it never asserts from an
          unloaded read: `open` needs `loaded`, so the row cannot draw before
          the preferences land. Move it outside the fold and both return. */}
      <div class="settings-row settings-row-child" data-search-anchor="models:typesafe-usage">
        <span class="settings-row-label">
          In use by
          <span class="list-row-details list-row-details-prose">
            {inUseBy.length === 0
              ? 'Nothing yet. Every classification still runs on its chat model.'
              : inUseBy.join(', ')}
          </span>
        </span>
      </div>
    </ProviderBlockFrame>
  );
}
