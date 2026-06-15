import { useRef, useEffect, useState } from 'preact/hooks';
import { activeInlineForm, credentials, showToast } from '../../store/store';
import {
  closeCredentialForm,
  submitNewCredential,
  submitCredentialEdit,
  loadCredentials,
} from '../../store/actions/credentials';
import { loadedOr, toFailed } from '../../store/types';
import type { AuthType, CredentialInfo, CredentialRequest, EmailAccountInfo, Loadable } from '../../store/types';
import { getCredentialValue, getEmailAccount } from '../../api/client';
import type { UpdateCredentialBody } from '../../api/client/settings';
import { Dropdown } from '../shared/Dropdown';
import { SecretInput } from '../shared/SecretInput';
import { isMobile } from '../../utils/viewport';
import { pickCredentialAutofocus } from './credentialAutofocus';
import { parseSecret, buildSecret, emptyFields, type CredentialFields } from './credentialSecret';
import { LoadableError } from '../shared/LoadableError';
import { DelayedSpinner } from '../shared/DelayedSpinner';

/** Strip the `email:` prefix the engine uses for email-account credentials. */
function emailAccountName(service: string): string {
  return service.startsWith('email:') ? service.slice('email:'.length) : service;
}

export function CredentialModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'credential') return null;

  // Editing pre-loads the stored secret (+ email settings) so the uncontrolled
  // inputs initialise from real values; create/request needs no async load.
  // Keyed by the credential: switching edit targets remounts the loader, so a
  // stale `data` from the previous credential can never seed the next form.
  if (form.editing) return <CredentialEditLoader key={form.editing} editing={form.editing} />;

  return (
    <CredentialFormInner
      editing={undefined}
      request={form.request}
      existingCred={null}
      initialFields={emptyFields()}
      emailInfo={null}
    />
  );
}

interface EditData {
  fields: CredentialFields;
  emailInfo: EmailAccountInfo | null;
}

/** Loads the current secret value + email settings for an edit, then renders
 *  the form pre-filled. The secret is fetched via the same endpoint the copy
 *  buttons use — no new exposure. */
function CredentialEditLoader({ editing }: { editing: string }) {
  const credLoadable = credentials.value;
  const [data, setData] = useState<Loadable<EditData>>({ status: 'not-loaded' });

  useEffect(() => {
    if (credLoadable.status === 'not-loaded') void loadCredentials();
  }, [credLoadable.status]);

  const creds = loadedOr(credLoadable, []);
  const existingCred =
    credLoadable.status === 'loaded' ? creds.find((c) => c.service_name === editing) ?? null : null;

  useEffect(() => {
    if (credLoadable.status !== 'loaded') return;
    if (!existingCred) {
      setData({ status: 'failed', error: `Credential "${editing}" not found` });
      return;
    }
    let cancelled = false;
    setData({ status: 'loading' });
    (async () => {
      try {
        const secret = await getCredentialValue(editing);
        const fields = parseSecret(existingCred.auth_type, secret.auth_value);
        const emailInfo =
          existingCred.auth_type === 'email_password'
            ? await getEmailAccount(emailAccountName(editing))
            : null;
        if (!cancelled) setData({ status: 'loaded', data: { fields, emailInfo } });
      } catch (e) {
        if (!cancelled) setData(toFailed(e));
      }
    })();
    return () => {
      cancelled = true;
    };
    // existingCred identity changes only with the credentials list / editing key.
  }, [credLoadable.status, editing, existingCred?.auth_type]);

  if (credLoadable.status === 'failed') {
    return (
      <div class="inline-form">
        <LoadableError error={credLoadable.error} noun="credentials" />
      </div>
    );
  }
  if (data.status === 'failed') {
    return (
      <div class="inline-form">
        <LoadableError error={data.error} noun="credential" />
      </div>
    );
  }
  if (credLoadable.status !== 'loaded' || data.status !== 'loaded' || !existingCred) {
    return (
      <div class="inline-form">
        <DelayedSpinner />
      </div>
    );
  }

  return (
    <CredentialFormInner
      key={editing}
      editing={editing}
      request={undefined}
      existingCred={existingCred}
      initialFields={data.data.fields}
      emailInfo={data.data.emailInfo}
    />
  );
}

interface CredentialFormInnerProps {
  editing?: string;
  request?: CredentialRequest;
  existingCred: CredentialInfo | null;
  initialFields: CredentialFields;
  emailInfo: EmailAccountInfo | null;
}

function CredentialFormInner({
  editing,
  request,
  existingCred,
  initialFields,
  emailInfo,
}: CredentialFormInnerProps) {
  const initialService = editing || request?.service || '';
  const initialBaseUrl = existingCred?.base_url || request?.base_url || '';
  const initialAuthType = existingCred?.auth_type || request?.auth_type || 'api_key';
  const initialAuthHeader = existingCred?.auth_header || 'Authorization';
  const serviceDisabled = !!editing || !!request?.service;

  const instructions = request?.prompt || null;
  const oauthDefaults = request?.defaults;
  // True when the agent pre-filled at least one endpoint from the oauth-providers
  // knowhow. Drives whether the endpoint section is collapsed + the label copy.
  const hasPrefilledEndpoints = !!(oauthDefaults?.auth_url || oauthDefaults?.token_url);
  // On edit, the OAuth endpoints come from the stored secret; on a request they
  // come from the agent-supplied defaults.
  const initialAuthUrl = initialFields.authUrl || oauthDefaults?.auth_url || '';
  const initialTokenUrl = initialFields.tokenUrl || oauthDefaults?.token_url || '';
  const initialUserinfoUrl = initialFields.userinfoUrl || oauthDefaults?.userinfo_url || '';
  const initialScopes = initialFields.scopes || oauthDefaults?.scopes || '';

  const [selectedAuthType, setSelectedAuthType] = useState<AuthType>(initialAuthType);

  const serviceRef = useRef<HTMLInputElement>(null);
  const baseUrlRef = useRef<HTMLInputElement>(null);
  const authHeaderRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef<HTMLInputElement>(null);
  const usernameRef = useRef<HTMLInputElement>(null);
  const passwordRef = useRef<HTMLInputElement>(null);
  const clientIdRef = useRef<HTMLInputElement>(null);
  const clientSecretRef = useRef<HTMLInputElement>(null);
  const authUrlRef = useRef<HTMLInputElement>(null);
  const tokenUrlRef = useRef<HTMLInputElement>(null);
  const userinfoUrlRef = useRef<HTMLInputElement>(null);
  const scopesRef = useRef<HTMLInputElement>(null);
  // Email server settings
  const emailAddressRef = useRef<HTMLInputElement>(null);
  const imapHostRef = useRef<HTMLInputElement>(null);
  const imapPortRef = useRef<HTMLInputElement>(null);
  const smtpHostRef = useRef<HTMLInputElement>(null);
  const smtpPortRef = useRef<HTMLInputElement>(null);
  const emailUsernameRef = useRef<HTMLInputElement>(null);
  const useTlsRef = useRef<HTMLInputElement>(null);
  const requireConfirmRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!request) return;
    const target = pickCredentialAutofocus(selectedAuthType, isMobile());
    if (target === 'username') usernameRef.current?.focus();
    else if (target === 'clientId') clientIdRef.current?.focus();
    else if (target === 'authValue') valueRef.current?.focus();
  }, [request, selectedAuthType]);

  const isEmailPassword = selectedAuthType === 'email_password';
  const isPassword = selectedAuthType === 'password';
  const isOAuthClient = selectedAuthType === 'oauth_client';
  const isSingleValue = !isEmailPassword && !isPassword && !isOAuthClient;
  // Email server settings only make sense when editing an existing account;
  // the create/request flow just collects the password.
  const showEmailSettings = isEmailPassword && !!editing;

  function getAuthLabel(authType: string): string {
    switch (authType) {
      case 'api_key': return 'API Key';
      case 'bearer': return 'Bearer Token';
      case 'basic': return 'Username:Password';
      case 'email_password': return 'App Password';
      default: return 'Token';
    }
  }

  function getAuthPlaceholder(authType: string): string {
    switch (authType) {
      case 'api_key': return 'Enter your API key';
      case 'bearer': return 'Enter your bearer token';
      case 'basic': return 'Enter as username:password';
      case 'email_password': return 'Enter your app password';
      default: return 'Enter your token or API key';
    }
  }

  /** Collect the secret field values from the inputs into a CredentialFields. */
  function collectFields(): CredentialFields {
    return {
      value: valueRef.current?.value || '',
      username: usernameRef.current?.value || '',
      password: passwordRef.current?.value || '',
      clientId: clientIdRef.current?.value || '',
      clientSecret: clientSecretRef.current?.value || '',
      authUrl: authUrlRef.current?.value.trim() || '',
      tokenUrl: tokenUrlRef.current?.value.trim() || '',
      userinfoUrl: userinfoUrlRef.current?.value.trim() || '',
      scopes: scopesRef.current?.value.trim() || '',
    };
  }

  async function handleSubmit(e: Event) {
    e.preventDefault();
    const service = serviceRef.current?.value.trim() || '';
    const baseUrl = baseUrlRef.current?.value.trim() || '';
    const authType = selectedAuthType;
    const fields = collectFields();

    if (authType === 'oauth_client') {
      // Pair validation: if either custom endpoint URL is given, both must be.
      if ((fields.authUrl && !fields.tokenUrl) || (!fields.authUrl && fields.tokenUrl)) {
        showToast('Authorization URL and Token URL must both be provided for a custom OAuth provider.', 'error');
        return;
      }
      if (!editing && (!fields.clientId || !fields.clientSecret)) return;
    } else if (authType === 'password') {
      if (!editing && (!fields.username || !fields.password)) return;
    }

    const authValue = buildSecret(authType, fields);

    if (editing) {
      const email = showEmailSettings
        ? {
            email_address: emailAddressRef.current?.value.trim() || '',
            imap_host: imapHostRef.current?.value.trim() || '',
            imap_port: parseInt(imapPortRef.current?.value || '', 10) || 993,
            smtp_host: smtpHostRef.current?.value.trim() || '',
            smtp_port: parseInt(smtpPortRef.current?.value || '', 10) || 465,
            username: emailUsernameRef.current?.value.trim() || '',
            use_tls: useTlsRef.current?.checked ?? true,
            require_send_confirmation: requireConfirmRef.current?.checked ?? true,
          }
        : undefined;

      if (email && (!email.email_address || !email.imap_host || !email.smtp_host)) {
        showToast('Email address, IMAP host and SMTP host are required.', 'error');
        return;
      }

      const body: UpdateCredentialBody = {
        base_url: existingCred?.base_url || baseUrl,
        auth_type: authType,
        auth_header: authHeaderRef.current?.value.trim() || initialAuthHeader,
        auth_value: authValue,
        email,
      };
      await submitCredentialEdit(editing, body);
    } else {
      await submitNewCredential(service, baseUrl, authType, authValue);
    }
  }

  return (
    <div class="inline-form">
      {instructions && (
        <blockquote class="credential-instructions">{instructions}</blockquote>
      )}
      <form onSubmit={handleSubmit}>
        {!isEmailPassword && (
          <>
            <div class="form-group">
              <label>Service Name</label>
              <input
                ref={serviceRef}
                type="text"
                value={initialService}
                disabled={serviceDisabled}
                placeholder="e.g. GitHub, Jira"
                required
              />
            </div>
            <div class="form-group">
              <label>Base URL</label>
              <input
                ref={baseUrlRef}
                type="url"
                value={initialBaseUrl}
                placeholder="e.g. https://api.github.com"
                required
              />
            </div>
            <div class="form-group">
              <label>Auth Type</label>
              <Dropdown
                options={[
                  { value: 'api_key', label: 'API Key' },
                  { value: 'bearer', label: 'Bearer Token' },
                  { value: 'basic', label: 'Basic Auth' },
                  { value: 'password', label: 'Password' },
                  { value: 'oauth_client', label: 'OAuth Client' },
                ]}
                value={selectedAuthType}
                onChange={(v) => setSelectedAuthType(v as AuthType)}
              />
            </div>
          </>
        )}
        {isEmailPassword && (
          <>
            <input ref={serviceRef} type="hidden" value={initialService} />
            <input ref={baseUrlRef} type="hidden" value={initialBaseUrl} />
          </>
        )}
        {showEmailSettings && (
          <>
            <div class="form-group">
              <label>Email Address</label>
              <input ref={emailAddressRef} type="email" defaultValue={emailInfo?.email_address || ''} placeholder="name@example.com" required />
            </div>
            <div class="form-group">
              <label>IMAP Host</label>
              <input ref={imapHostRef} type="text" defaultValue={emailInfo?.imap_host || ''} placeholder="imap.example.com" required />
            </div>
            <div class="form-group">
              <label>IMAP Port</label>
              <input ref={imapPortRef} type="number" defaultValue={String(emailInfo?.imap_port ?? 993)} placeholder="993" />
            </div>
            <div class="form-group">
              <label>SMTP Host</label>
              <input ref={smtpHostRef} type="text" defaultValue={emailInfo?.smtp_host || ''} placeholder="smtp.example.com" required />
            </div>
            <div class="form-group">
              <label>SMTP Port</label>
              <input ref={smtpPortRef} type="number" defaultValue={String(emailInfo?.smtp_port ?? 465)} placeholder="465" />
            </div>
            <div class="form-group">
              <label>Username</label>
              <input ref={emailUsernameRef} type="text" defaultValue={emailInfo?.username || ''} placeholder="usually the full email address" />
            </div>
            <div class="form-group">
              <label class="form-checkbox-row">
                <input ref={useTlsRef} type="checkbox" defaultChecked={emailInfo?.use_tls ?? true} />
                Use TLS/SSL
              </label>
            </div>
            <div class="form-group">
              <label class="form-checkbox-row">
                <input ref={requireConfirmRef} type="checkbox" defaultChecked={emailInfo?.require_send_confirmation ?? true} />
                Require confirmation before sending mail
              </label>
            </div>
          </>
        )}
        {isOAuthClient ? (
          <>
            <div class="form-group">
              <label>Client ID</label>
              <input
                ref={clientIdRef}
                type="text"
                defaultValue={initialFields.clientId}
                placeholder="Enter your OAuth Client ID"
                required={!editing}
              />
            </div>
            <div class="form-group">
              <label>Client Secret</label>
              <SecretInput
                inputRef={clientSecretRef}
                defaultValue={initialFields.clientSecret}
                placeholder="Enter your OAuth Client Secret"
                required={!editing}
              />
            </div>
            <details class="credential-oauth-endpoints" open={!hasPrefilledEndpoints}>
              <summary>
                {hasPrefilledEndpoints
                  ? 'OAuth endpoints (pre-filled — edit if needed)'
                  : 'OAuth endpoints (required)'}
              </summary>
              <div class="form-group">
                <label>Authorization URL</label>
                <input ref={authUrlRef} type="url" defaultValue={initialAuthUrl} placeholder="https://accounts.example.com/authorize" />
              </div>
              <div class="form-group">
                <label>Token URL</label>
                <input ref={tokenUrlRef} type="url" defaultValue={initialTokenUrl} placeholder="https://accounts.example.com/api/token" />
              </div>
              <div class="form-group">
                <label>Userinfo URL <span class="form-hint">(optional)</span></label>
                <input ref={userinfoUrlRef} type="url" defaultValue={initialUserinfoUrl} placeholder="https://api.example.com/v1/me" />
              </div>
              <div class="form-group">
                <label>Default Scopes <span class="form-hint">(optional, space-separated)</span></label>
                <input ref={scopesRef} type="text" defaultValue={initialScopes} placeholder="read write user.read" />
              </div>
            </details>
          </>
        ) : isPassword ? (
          <>
            <div class="form-group">
              <label>Username</label>
              <input
                ref={usernameRef}
                type="text"
                defaultValue={initialFields.username}
                placeholder="Enter your username"
                required={!editing}
              />
            </div>
            <div class="form-group">
              <label>Password</label>
              <SecretInput
                inputRef={passwordRef}
                defaultValue={initialFields.password}
                placeholder="Enter your password"
                required={!editing}
              />
            </div>
          </>
        ) : (
          <div class="form-group">
            <label>{getAuthLabel(selectedAuthType)}</label>
            <SecretInput
              inputRef={valueRef}
              defaultValue={initialFields.value}
              placeholder={getAuthPlaceholder(selectedAuthType)}
              required={!editing}
            />
          </div>
        )}
        {editing && isSingleValue && (
          <details class="credential-oauth-endpoints">
            <summary>Advanced</summary>
            <div class="form-group">
              <label>Auth Header</label>
              <input ref={authHeaderRef} type="text" defaultValue={initialAuthHeader} placeholder="Authorization" />
            </div>
          </details>
        )}
        <div class="form-actions">
          <button type="button" class="btn-cancel" onClick={closeCredentialForm}>
            Cancel
          </button>
          <button type="submit" class="btn-save">
            Save
          </button>
        </div>
      </form>
    </div>
  );
}
