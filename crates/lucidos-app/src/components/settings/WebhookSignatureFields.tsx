
import { Dropdown, DropdownSkeleton } from '../shared/Dropdown';
import { LoadingFade } from '../shared/LoadingFade';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
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

export interface SignatureDraft {
  scheme: WebhookScheme;
  source: SecretSource;
  credential: string;
  secret: string;
  custom: CustomFields;
}

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

export function draftToSigningSecret(draft: SignatureDraft): WebhookSigningSecret | undefined {
  if (draft.source === 'generate') return { mode: 'generate' };
  if (draft.source === 'paste') return { mode: 'provided', value: draft.secret };
  return undefined;
}

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

function SavedCredentialControl({ credentials, draft, patch }: {
  credentials: Loadable<CredentialInfo[]>;
  draft: SignatureDraft;
  patch: (change: Partial<SignatureDraft>) => void;
}) {
  const showLoading = useDelayedLoading(credentials);
  if (credentials.status === 'failed') {
    return <LoadableError noun="credentials" error={credentials.error} />;
  }
  const saved = signableCredentials(credentials);
  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<DropdownSkeleton w="8rem" />}>
      {saved?.length === 0 && (
        <div class="settings-section-desc">
          No saved credentials yet. Generate one here, or add it in Accounts.
        </div>
      )}
      {!!saved?.length && (
        <Dropdown
          options={saved.map((c) => ({ value: c.service_name, label: c.service_name }))}
          value={draft.credential}
          onChange={(v) => patch({ credential: v })}
          placeholder="Pick a credential"
        />
      )}
    </LoadingFade>
  );
}

interface Props {
  draft: SignatureDraft;
  credentials: Loadable<CredentialInfo[]>;
  onChange: (draft: SignatureDraft) => void;
}

export function WebhookSignatureFields({ draft, credentials, onChange }: Props) {
  const preset = presetFor(draft.scheme);

  function patch(change: Partial<SignatureDraft>) {
    onChange({ ...draft, ...change });
  }

  function patchCustom(change: Partial<CustomFields>) {
    onChange({ ...draft, custom: { ...draft.custom, ...change } });
  }

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
          <SavedCredentialControl credentials={credentials} draft={draft} patch={patch} />
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
              placeholder="Seconds. Empty uses the default window if the template signs {timestamp}."
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
