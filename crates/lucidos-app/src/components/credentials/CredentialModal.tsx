import { useRef, useEffect, useState } from 'preact/hooks';
import { activeInlineForm, credentials } from '../../store/store';
import { closeCredentialForm, submitCredential } from '../../store/actions/credentials';
import { loadedOr } from '../../store/types';
import type { AuthType, CredentialInfo, CredentialRequest } from '../../store/types';
import { Dropdown } from '../shared/Dropdown';

export function CredentialModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'credential') return null;

  const editing = form.editing;
  const request = form.request;

  // When editing, wait for credentials to load before rendering the form —
  // otherwise useState hooks initialise with empty values that never update.
  if (editing && credentials.value.status !== 'loaded') {
    return <div class="inline-form"><div class="empty">Loading...</div></div>;
  }

  const creds = loadedOr(credentials.value, []);
  const existingCred = editing
    ? creds.find((c) => c.service_name === editing) ?? null
    : null;

  return (
    <CredentialFormInner
      key={editing ?? 'new'}
      editing={editing}
      request={request}
      existingCred={existingCred}
    />
  );
}

interface CredentialFormInnerProps {
  editing?: string;
  request?: CredentialRequest;
  existingCred: CredentialInfo | null;
}

function CredentialFormInner({ editing, request, existingCred }: CredentialFormInnerProps) {
  // Pre-fill from editing or request
  const initialService = editing || request?.service || '';
  const initialBaseUrl = existingCred?.base_url || request?.base_url || '';
  const initialAuthType = existingCred?.auth_type || request?.auth_type || 'api_key';
  const serviceDisabled = !!editing || !!request?.service;

  const instructions = request?.prompt || null;

  const [selectedAuthType, setSelectedAuthType] = useState<AuthType>(initialAuthType);

  const serviceRef = useRef<HTMLInputElement>(null);
  const baseUrlRef = useRef<HTMLInputElement>(null);
  const authValueRef = useRef<HTMLInputElement>(null);
  const usernameRef = useRef<HTMLInputElement>(null);
  const passwordRef = useRef<HTMLInputElement>(null);
  const clientIdRef = useRef<HTMLInputElement>(null);
  const clientSecretRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (request) {
      if (selectedAuthType === 'password') {
        usernameRef.current?.focus();
      } else if (selectedAuthType === 'oauth_client') {
        clientIdRef.current?.focus();
      } else {
        authValueRef.current?.focus();
      }
    }
  }, [request, selectedAuthType]);

  const isEmailPassword = selectedAuthType === 'email_password';
  const isPassword = selectedAuthType === 'password';
  const isOAuthClient = selectedAuthType === 'oauth_client';

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
    if (editing) return 'Enter new value (leave empty to keep current)';
    switch (authType) {
      case 'api_key': return 'Enter your API key';
      case 'bearer': return 'Enter your bearer token';
      case 'basic': return 'Enter as username:password';
      case 'email_password': return 'Enter your app password';
      default: return 'Enter your token or API key';
    }
  }

  async function handleSubmit(e: Event) {
    e.preventDefault();
    const service = serviceRef.current?.value.trim() || '';
    const baseUrl = baseUrlRef.current?.value.trim() || '';
    const authType = selectedAuthType;

    let authValue: string;
    if (authType === 'oauth_client') {
      const clientId = clientIdRef.current?.value || '';
      const clientSecret = clientSecretRef.current?.value || '';
      if (!editing && (!clientId || !clientSecret)) return;
      if (editing && !clientId && !clientSecret) {
        authValue = '';
      } else {
        authValue = JSON.stringify({ client_id: clientId, client_secret: clientSecret });
      }
    } else if (authType === 'password') {
      const username = usernameRef.current?.value || '';
      const password = passwordRef.current?.value || '';
      if (!editing && (!username || !password)) return;
      if (editing && !username && !password) {
        authValue = '';
      } else {
        authValue = JSON.stringify({ username, password });
      }
    } else {
      authValue = authValueRef.current?.value || '';
    }

    await submitCredential(service, baseUrl, authType, authValue);
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
        {isOAuthClient ? (
          <>
            <div class="form-group">
              <label>Client ID</label>
              <input
                ref={clientIdRef}
                type="text"
                placeholder={editing ? 'Enter new Client ID (leave empty to keep current)' : 'Enter your OAuth Client ID'}
                required={!editing}
              />
            </div>
            <div class="form-group">
              <label>Client Secret</label>
              <input
                ref={clientSecretRef}
                type="password"
                placeholder={editing ? 'Enter new Client Secret (leave empty to keep current)' : 'Enter your OAuth Client Secret'}
                required={!editing}
              />
            </div>
          </>
        ) : isPassword ? (
          <>
            <div class="form-group">
              <label>Username</label>
              <input
                ref={usernameRef}
                type="text"
                placeholder={editing ? 'Enter new username (leave empty to keep current)' : 'Enter your username'}
                required={!editing}
              />
            </div>
            <div class="form-group">
              <label>Password</label>
              <input
                ref={passwordRef}
                type="password"
                placeholder={editing ? 'Enter new password (leave empty to keep current)' : 'Enter your password'}
                required={!editing}
              />
            </div>
          </>
        ) : (
          <div class="form-group">
            <label>{getAuthLabel(selectedAuthType)}</label>
            <input
              ref={authValueRef}
              type="password"
              placeholder={getAuthPlaceholder(selectedAuthType)}
              required={!editing}
            />
          </div>
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
