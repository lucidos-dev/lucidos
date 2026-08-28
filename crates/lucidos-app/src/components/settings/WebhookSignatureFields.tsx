/**
 * The signature editor, shared by the create form and by an existing row.
 *
 * One component so the presets and the raw fields cannot drift between the two
 * surfaces. It owns no server state: the parent holds a `SignatureDraft` and
 * gets it back on every change, which keeps the whole thing testable through
 * the pure helpers in `webhookSignature.ts`.
 */

import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import type { CredentialInfo, Loadable } from '../../store/types';
import type { WebhookHmac, WebhookSigningSecret } from '../../api/client';
import {
  presetFor,
  signableCredentials,
  suggestedCredentialName,
  SCHEME_PRESETS,
  type SecretSource,
  type WebhookScheme,
} from './webhookSignature';

/** What the user has picked so far, before it becomes a request.
 *
 *  `credential` means two things by design, and which one depends on `source`.
 *  For `saved` it names a row that exists. For the other two it names the row
 *  about to be written. Both end up as `hmac.credential`, so one field carries
 *  it either way. */
export interface SignatureDraft {
  scheme: WebhookScheme;
  source: SecretSource;
  credential: string;
  /** The pasted value. Untrimmed on purpose: the engine refuses surrounding
   *  whitespace by name rather than silently stripping it. */
  secret: string;
  /** Only read under the custom scheme, which pins nothing. */
  custom: CustomFields;
}

/** The raw `HmacConfig` fields, as strings the form can hold. */
export interface CustomFields {
  signature_header: string;
  algorithm: 'sha256' | 'sha1';
  encoding: 'hex' | 'base64';
  prefix: string;
  signature_key: string;
  timestamp_header: string;
  timestamp_key: string;
  template: string;
  tolerance_secs: string;
}

function emptyCustomFields(): CustomFields {
  return {
    signature_header: '',
    algorithm: 'sha256',
    encoding: 'hex',
    prefix: '',
    signature_key: '',
    timestamp_header: '',
    timestamp_key: '',
    template: '{body}',
    tolerance_secs: '',
  };
}

/** A draft for a hook that does not sign yet. */
export function newSignatureDraft(hookName: string): SignatureDraft {
  const scheme: WebhookScheme = 'github';
  return {
    scheme,
    source: presetFor(scheme).defaultSource,
    credential: suggestedCredentialName(hookName, scheme),
    secret: '',
    custom: emptyCustomFields(),
  };
}

/** A draft that reopens what a hook already stores.
 *
 *  The source starts at `saved`, whatever the scheme's default is. The hook
 *  already has a secret, so proposing a new one would be proposing to replace
 *  a working credential nobody asked about. */
export function draftFromHmac(hmac: WebhookHmac, scheme: WebhookScheme): SignatureDraft {
  return {
    scheme,
    source: 'saved',
    credential: hmac.credential,
    secret: '',
    custom: {
      signature_header: hmac.signature_header,
      algorithm: hmac.algorithm ?? 'sha256',
      encoding: hmac.encoding ?? 'hex',
      prefix: hmac.prefix ?? '',
      signature_key: hmac.signature_key ?? '',
      timestamp_header: hmac.timestamp_header ?? '',
      timestamp_key: hmac.timestamp_key ?? '',
      template: hmac.template ?? '{body}',
      tolerance_secs: hmac.tolerance_secs == null ? '' : String(hmac.tolerance_secs),
    },
  };
}

/** The `hmac` block a draft becomes, or `null` when it is not ready to send. */
export function draftToHmac(draft: SignatureDraft): WebhookHmac | null {
  const credential = draft.credential.trim();
  if (!credential) return null;
  const preset = presetFor(draft.scheme);
  if (preset.hmac) return { credential, ...preset.hmac };

  const f = draft.custom;
  if (!f.signature_header.trim() || !f.template.includes('{body}')) return null;
  const tolerance = f.tolerance_secs.trim();
  return {
    credential,
    signature_header: f.signature_header.trim(),
    algorithm: f.algorithm,
    encoding: f.encoding,
    ...(f.prefix ? { prefix: f.prefix } : {}),
    ...(f.signature_key.trim() ? { signature_key: f.signature_key.trim() } : {}),
    ...(f.timestamp_header.trim() ? { timestamp_header: f.timestamp_header.trim() } : {}),
    ...(f.timestamp_key.trim() ? { timestamp_key: f.timestamp_key.trim() } : {}),
    template: f.template,
    ...(tolerance ? { tolerance_secs: Number(tolerance) } : {}),
  };
}

/** The `signing_secret` a draft becomes, or `undefined` to name a saved one. */
export function draftToSigningSecret(draft: SignatureDraft): WebhookSigningSecret | undefined {
  if (draft.source === 'generate') return { mode: 'generate' };
  // Sent verbatim. The engine refuses surrounding whitespace by name, which
  // beats trimming a value that may legitimately need it.
  if (draft.source === 'paste') return { mode: 'provided', value: draft.secret };
  return undefined;
}

/** Why this draft cannot be saved yet, or `null` when it can. */
export function draftBlocker(draft: SignatureDraft): string | null {
  if (!draft.credential.trim()) return 'Name the credential this hook verifies with';
  if (draft.source === 'paste' && !draft.secret) return 'Paste the secret the sender issued';
  if (!draftToHmac(draft)) {
    return 'A custom scheme needs a signature header and a template containing {body}';
  }
  return null;
}

const SOURCE_LABELS: Record<SecretSource, string> = {
  saved: 'Use a saved credential',
  generate: 'Generate a secret',
  paste: 'Paste the sender’s secret',
};

/** The credential picker, or what to say instead of one.
 *
 *  All four `Loadable` states get their own answer. Collapsing them read as
 *  "you have none saved" while the list was merely in flight, and stayed on
 *  screen for good when the fetch failed. */
function savedCredentialControl(
  credentials: Loadable<CredentialInfo[]>,
  saved: CredentialInfo[] | null,
  draft: SignatureDraft,
  patch: (change: Partial<SignatureDraft>) => void,
) {
  if (credentials.status === 'failed') {
    return <LoadableError noun="credentials" error={credentials.error} />;
  }
  if (saved === null) {
    return <div class="settings-section-desc">Loading credentials...</div>;
  }
  if (saved.length === 0) {
    return (
      <div class="settings-section-desc">
        No saved credentials yet. Generate one here, or add it in Accounts.
      </div>
    );
  }
  return (
    <Dropdown
      options={saved.map((c) => ({ value: c.service_name, label: c.service_name }))}
      value={draft.credential}
      onChange={(v) => patch({ credential: v })}
      placeholder="Pick a credential"
    />
  );
}

interface Props {
  draft: SignatureDraft;
  /** The whole `Loadable`, so the picker can tell "none saved" from "not
   *  loaded yet" and from "the fetch failed". */
  credentials: Loadable<CredentialInfo[]>;
  onChange: (draft: SignatureDraft) => void;
}

export function WebhookSignatureFields({ draft, credentials, onChange }: Props) {
  const preset = presetFor(draft.scheme);
  const saved = signableCredentials(credentials);

  function patch(change: Partial<SignatureDraft>) {
    onChange({ ...draft, ...change });
  }

  function patchCustom(change: Partial<CustomFields>) {
    onChange({ ...draft, custom: { ...draft.custom, ...change } });
  }

  /** Switching scheme re-picks the source, because which sources are even
   *  possible depends on the sender. A Slack hook cannot generate. */
  function pickScheme(scheme: WebhookScheme) {
    const next = presetFor(scheme);
    const source = next.secretSources.includes(draft.source)
      ? draft.source
      : next.defaultSource;
    onChange({ ...draft, scheme, source });
  }

  return (
    <div class="webhook-signature-fields">
      <div class="webhook-signature-row">
        <label>Sender</label>
        <Dropdown
          options={SCHEME_PRESETS.map((p) => ({ value: p.scheme, label: p.label }))}
          value={draft.scheme}
          onChange={(v) => pickScheme(v as WebhookScheme)}
        />
      </div>

      <div class="webhook-signature-row">
        <label>Secret</label>
        <Dropdown
          options={preset.secretSources.map((s) => ({ value: s, label: SOURCE_LABELS[s] }))}
          value={draft.source}
          onChange={(v) => patch({ source: v as SecretSource })}
        />
      </div>

      {draft.source === 'saved' ? (
        <div class="webhook-signature-row">
          <label>Credential</label>
          {savedCredentialControl(credentials, saved, draft, patch)}
        </div>
      ) : (
        <div class="webhook-signature-row">
          <label>Save it as</label>
          <input
            class="device-name-input"
            type="text"
            placeholder="Credential name (e.g. deploys-github)"
            value={draft.credential}
            onInput={(e) => patch({ credential: (e.target as HTMLInputElement).value })}
          />
        </div>
      )}

      {draft.source === 'paste' && (
        <div class="webhook-signature-row">
          <label>Value</label>
          <input
            class="device-name-input"
            type="password"
            placeholder="Paste the secret the sender issued"
            value={draft.secret}
            onInput={(e) => patch({ secret: (e.target as HTMLInputElement).value })}
          />
        </div>
      )}

      <div class="settings-section-desc">{preset.hint}</div>

      {draft.scheme === 'custom' && (
        <>
          <div class="webhook-signature-row">
            <label>Signature header</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="e.g. X-Signature"
              value={draft.custom.signature_header}
              onInput={(e) =>
                patchCustom({ signature_header: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Signed string</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="{body}"
              value={draft.custom.template}
              onInput={(e) => patchCustom({ template: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Digest</label>
            <div class="webhook-signature-pair">
              <Dropdown
                options={[
                  { value: 'sha256', label: 'SHA-256' },
                  { value: 'sha1', label: 'SHA-1' },
                ]}
                value={draft.custom.algorithm}
                onChange={(v) => patchCustom({ algorithm: v as 'sha256' | 'sha1' })}
              />
              <Dropdown
                options={[
                  { value: 'hex', label: 'Hex' },
                  { value: 'base64', label: 'Base64' },
                ]}
                value={draft.custom.encoding}
                onChange={(v) => patchCustom({ encoding: v as 'hex' | 'base64' })}
              />
            </div>
          </div>
          <div class="webhook-signature-row">
            <label>Prefix</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="Stripped off the header value, e.g. sha256="
              value={draft.custom.prefix}
              onInput={(e) => patchCustom({ prefix: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Signature key</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="Read out of a k=v header instead, e.g. v1"
              value={draft.custom.signature_key}
              onInput={(e) =>
                patchCustom({ signature_key: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Timestamp header</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="Only for a scheme signing a timestamp"
              value={draft.custom.timestamp_header}
              onInput={(e) =>
                patchCustom({ timestamp_header: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Timestamp key</label>
            <input
              class="device-name-input"
              type="text"
              placeholder="Or the key holding it inside the signature header"
              value={draft.custom.timestamp_key}
              onInput={(e) =>
                patchCustom({ timestamp_key: (e.target as HTMLInputElement).value })}
            />
          </div>
          <div class="webhook-signature-row">
            <label>Replay window</label>
            <input
              class="device-name-input"
              type="number"
              placeholder="Seconds. Empty skips the check, which suits a scheme with no timestamp."
              value={draft.custom.tolerance_secs}
              onInput={(e) =>
                patchCustom({ tolerance_secs: (e.target as HTMLInputElement).value })}
            />
          </div>
        </>
      )}
    </div>
  );
}
