import { useRef, useEffect, useState } from 'preact/hooks';
import { activeInlineForm, credentials, knownOAuthProviders, showToast } from '../../store/store';
import {
  closeCredentialForm,
  submitNewCredential,
  submitCredentialEdit,
  submitRequestedCredential,
  loadCredentials,
} from '../../store/actions/credentials';
import { loadKnownOAuthProviders } from '../../store/actions/oauth';
import {
  describeMissingFields,
  oauthClientSubmitError,
  rowForService,
  secretIsExpected,
} from './oauthClientForm';
import { ConsoleLink } from './providerConsoleHint';
import { FieldLabel } from '../shared/Explainer';
import { loadedOr, toFailed } from '../../store/types';
import type { AuthType, CredentialInfo, CredentialRequest, EmailAccountInfo, Loadable } from '../../store/types';
import { getCredentialValue, getEmailAccount } from '../../api/client';
import type { UpdateCredentialBody } from '../../api/client/settings';
import { Dropdown } from '../shared/Dropdown';
import { copyToClipboard } from '../../utils/clipboard';
import { SecretInput } from '../shared/SecretInput';
import { isMobile } from '../../utils/viewport';
import { pickCredentialAutofocus } from './credentialAutofocus';
import { parseSecret, buildSecret, emptyFields, type CredentialFields } from './credentialSecret';
import { LoadableError } from '../shared/LoadableError';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';

/** The `email_accounts.name` an `email_password` credential belongs to.
 *
 *  Normally the identity function: the service name IS the account name, since
 *  `auth_type` carries what the old `email:` prefix used to say. The strip is
 *  the frontend half of the `credential-email-prefix-fallback` temporary
 *  measure (`docs/temporary-measures.md`), for the rows the prefix migration had
 *  to leave alone because their bare name was already taken. Without it, opening
 *  such a credential for edit 404s on the server-settings fetch. Mirrors
 *  `EmailStore::account_name_for_credential`; remove the two together. */
function emailAccountName(serviceName: string): string {
  return serviceName.startsWith('email:') ? serviceName.slice('email:'.length) : serviceName;
}

export function CredentialModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'credential') return null;

  // Editing pre-loads the stored secret (+ email settings) so the uncontrolled
  // inputs initialise from real values; a plain create needs no async load.
  // Keyed by the credential: switching edit targets remounts the loader, so a
  // stale `data` from the previous credential can never seed the next form.
  if (form.editing) return <CredentialStoredLoader key={form.editing} credentialId={form.editing} />;

  // A REPAIR targets a credential that already exists, so it loads the stored
  // secret too. Saving rebuilds the whole `auth_value` from the form, so a form
  // seeded only with the request's `client_id` would drop every other stored
  // field on save: a confidential client would silently lose its
  // `client_secret` and start failing the token exchange, and a provider the
  // registry does not know would lose its endpoints, scopes and redirect
  // override as well. The stored values win; the request's registry defaults
  // fill only what is genuinely missing (see `initialAuthUrl` and friends).
  const repairing = form.request?.existing_credential_id;
  if (repairing) {
    return (
      <CredentialStoredLoader
        key={repairing}
        credentialId={repairing}
        request={form.request}
      />
    );
  }

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

/** Loads a credential's stored secret (+ email settings), then renders the form
 *  pre-filled. The secret is fetched via the same endpoint the copy buttons use,
 *  so this is no new exposure.
 *
 *  Serves the form's two "this row already exists" entry points, which differ
 *  only in what happens on save. An **edit** the user opened updates the row and
 *  stops. A **repair** was opened by the engine refusing to start an
 *  authorization, so it carries the `request` and saving continues that flow
 *  (`submitRequestedCredential`, routed to update by `existing_credential_id`).
 *  Passing `request` is what tells the two apart, and a repair deliberately
 *  leaves `editing` unset so the form applies the stricter create rules: the
 *  endpoints it reopened for are genuinely required.
 *
 *  `credentialId` is the row's `id`, not its service name: a name no longer
 *  identifies one row (an `oauth_client` registration may share it with an API
 *  key), and the form can change `auth_type`, so even name-plus-type could not
 *  name it. The service name for display comes off the resolved row instead. */
function CredentialStoredLoader({
  credentialId,
  request,
}: {
  credentialId: string;
  request?: CredentialRequest;
}) {
  const credLoadable = credentials.value;
  const [data, setData] = useState<Loadable<EditData>>({ status: 'not-loaded' });

  useEffect(() => {
    if (credLoadable.status === 'not-loaded') void loadCredentials();
  }, [credLoadable.status]);

  const creds = loadedOr(credLoadable, []);
  const existingCred =
    credLoadable.status === 'loaded' ? creds.find((c) => c.id === credentialId) ?? null : null;

  useEffect(() => {
    if (credLoadable.status !== 'loaded') return;
    if (!existingCred) {
      // No id in the message: it names nothing the user can look up.
      setData({ status: 'failed', error: 'Credential not found' });
      return;
    }
    let cancelled = false;
    setData({ status: 'loading' });
    (async () => {
      try {
        const secret = await getCredentialValue(credentialId);
        const fields = parseSecret(existingCred.auth_type, secret.auth_value);
        const emailInfo =
          existingCred.auth_type === 'email_password'
            ? await getEmailAccount(emailAccountName(existingCred.service_name))
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
  }, [credLoadable.status, credentialId, existingCred?.auth_type]);

  // Delay the spinner so a fast secret fetch never flashes it. The failed
  // branches below render first.
  const showLoading = useDelayedFlag(
    credLoadable.status !== 'loaded' || data.status !== 'loaded' || !existingCred,
  );

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
  if (showLoading) {
    return (
      <div class="inline-form">
        <div class="loading-spinner" />
      </div>
    );
  }
  if (credLoadable.status !== 'loaded' || data.status !== 'loaded' || !existingCred) {
    // Pre-delay window — keep the container mounted but show nothing yet.
    return <div class="inline-form" />;
  }

  return (
    <CredentialFormInner
      key={credentialId}
      editing={request ? undefined : credentialId}
      request={request}
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
  // `editing` is an id, so the name shown comes off the resolved row.
  const initialService = existingCred?.service_name || request?.service || '';
  const initialBaseUrl = existingCred?.base_url || request?.base_url || '';
  const initialAuthType = existingCred?.auth_type || request?.auth_type || 'api_key';
  const initialAuthHeader = existingCred?.auth_header || 'Authorization';
  const initialEnvVarName = existingCred?.env_var_name || request?.env_var_name || '';
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
  const initialUserinfoMethod =
    initialFields.userinfoMethod || oauthDefaults?.userinfo_method || 'GET';
  const initialAuthorizeParams =
    initialFields.authorizeParams || oauthDefaults?.authorize_params || '';
  const initialScopes = initialFields.scopes || oauthDefaults?.scopes || '';
  const initialRedirectUri = initialFields.redirectUri || oauthDefaults?.redirect_uri || '';

  const [selectedAuthType, setSelectedAuthType] = useState<AuthType>(initialAuthType);
  /** The Base URL field. State rather than a ref write, because the input is
   *  controlled and every re-render would otherwise restore `initialBaseUrl`. */
  const [baseUrl, setBaseUrl] = useState(initialBaseUrl);

  // The *OAuth provider registry*, for the console help and the derived-provider
  // picker. Absent or failed degrades to no help and no picker, which is the
  // form exactly as it was before the registry existed.
  const registryLoadable = knownOAuthProviders.value;
  const knownProviders =
    registryLoadable.status === 'loaded' ? registryLoadable.data.providers : [];
  const redirectUri =
    registryLoadable.status === 'loaded' ? registryLoadable.data.default_redirect_uri : '';
  const registryRow = rowForService(knownProviders, initialService);
  /** What the repair notice compares the engine's `missing` list against: the
   *  values the form is actually showing, which for a known provider are the
   *  registry's rather than the broken row's. */
  const repairNotice = describeMissingFields(request?.missing, {
    client_id: initialFields.clientId,
    auth_url: initialAuthUrl,
    token_url: initialTokenUrl,
  });
  /** A repair that reopened the form FOR the endpoints shows them, prefilled or
   *  not. Collapsing them is what made the autofill look like it never ran: the
   *  notice named two fields as the whole reason the form is on screen, and both
   *  sat behind a shut disclosure carrying the word "pre-filled". */
  const repairNamedEndpoints = !!request?.missing?.some(
    (m) => m === 'auth_url' || m === 'token_url',
  );
  /** Which known provider a derived name was told to run on. Empty until picked.
   *
   *  Declared BEFORE `expectsSecret`, which reads it. That read happens inside a
   *  `.find` callback, so TypeScript's used-before-declaration check does not
   *  see it (the reference is in a function body, and the compiler cannot know
   *  when one runs) while `.find` in fact invokes it synchronously: the wrong
   *  order threw a temporal-dead-zone `ReferenceError` on every render of this
   *  form, and neither `tsc` nor the suite caught it because nothing mounts the
   *  modal. */
  const [basedOn, setBasedOn] = useState('');
  /** The provider whose console this registration is actually made in.
   *
   *  For a derived name that is the BASE provider the user picked, not the
   *  service name: the app is registered with the base provider, so its console
   *  link, redirect URI, client type and permissions are the ones that apply.
   *  Keying the help off the service name alone left a derived connection with
   *  no console link and no URI to copy, at exactly the moment the user is
   *  standing in that console needing both. */
  const helpRow = registryRow ?? knownProviders.find((p) => p.id === basedOn);
  const expectsSecret = secretIsExpected(helpRow);

  useEffect(() => {
    if (registryLoadable.status === 'not-loaded') void loadKnownOAuthProviders();
  }, [registryLoadable.status]);

  /** Fill the endpoint fields from a base provider's row.
   *
   *  Writes through the refs rather than through state because the endpoint
   *  inputs are uncontrolled (they hold `defaultValue`), so re-rendering would
   *  not move them. */
  function applyBaseProvider(id: string) {
    setBasedOn(id);
    const row = knownProviders.find((p) => p.id === id);
    if (!row) return;
    setBaseUrl(row.base_url);
    if (authUrlRef.current) authUrlRef.current.value = row.auth_url;
    if (tokenUrlRef.current) tokenUrlRef.current.value = row.token_url;
    if (userinfoUrlRef.current) userinfoUrlRef.current.value = row.userinfo_url ?? '';
    if (authorizeParamsRef.current) authorizeParamsRef.current.value = row.authorize_params ?? '';
    if (redirectUriRef.current) redirectUriRef.current.value = row.redirect_uri ?? '';
    setUserinfoMethod(row.userinfo_method ?? 'GET');
  }

  function copyRedirectUri() {
    copyToClipboard(redirectUri, 'Redirect URI copied to clipboard');
  }

  const serviceRef = useRef<HTMLInputElement>(null);
  const baseUrlRef = useRef<HTMLInputElement>(null);
  const authHeaderRef = useRef<HTMLInputElement>(null);
  const envVarNameRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef<HTMLInputElement>(null);
  const usernameRef = useRef<HTMLInputElement>(null);
  const passwordRef = useRef<HTMLInputElement>(null);
  const clientIdRef = useRef<HTMLInputElement>(null);
  const clientSecretRef = useRef<HTMLInputElement>(null);
  const authUrlRef = useRef<HTMLInputElement>(null);
  const tokenUrlRef = useRef<HTMLInputElement>(null);
  const userinfoUrlRef = useRef<HTMLInputElement>(null);
  const [userinfoMethod, setUserinfoMethod] = useState<string>(initialUserinfoMethod);
  const authorizeParamsRef = useRef<HTMLInputElement>(null);
  const scopesRef = useRef<HTMLInputElement>(null);
  const redirectUriRef = useRef<HTMLInputElement>(null);
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
  const isSecret = selectedAuthType === 'secret';
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
      case 'secret': return 'Secret';
      default: return 'Token';
    }
  }

  function getAuthPlaceholder(authType: string): string {
    switch (authType) {
      case 'api_key': return 'Enter your API key';
      case 'bearer': return 'Enter your bearer token';
      case 'basic': return 'Enter as username:password';
      case 'email_password': return 'Enter your app password';
      case 'secret': return 'Paste the shared secret';
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
      userinfoMethod,
      authorizeParams: authorizeParamsRef.current?.value.trim() || '',
      scopes: scopesRef.current?.value.trim() || '',
      redirectUri: redirectUriRef.current?.value.trim() || '',
    };
  }

  async function handleSubmit(e: Event) {
    e.preventDefault();
    const service = serviceRef.current?.value.trim() || '';
    const baseUrl = baseUrlRef.current?.value.trim() || '';
    const authType = selectedAuthType;
    const fields = collectFields();
    // Custom env var name is offered for every type except email_password.
    const envVarName = isEmailPassword ? undefined : envVarNameRef.current?.value.trim() || undefined;

    if (authType === 'oauth_client') {
      // Genuinely enforced now, and it says which field. The old rule was a PAIR
      // check that both-blank passed, so a client with no endpoints saved fine
      // and failed on the next Connect with "Missing auth_url in OAuth
      // credentials". A blank client SECRET is still fine: it is how a public
      // client is expressed (PKCE), not an omission.
      const refusal = oauthClientSubmitError(fields, !!editing);
      if (refusal) {
        showToast(refusal, 'error');
        return;
      }
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
        env_var_name: envVarName,
      };
      await submitCredentialEdit(editing, body);
    } else if (request) {
      // A credential the ENGINE asked for. Routed apart from a plain create for
      // two reasons: a repair carries the id of the row it must update (creating
      // would make a second OAuth Client for one provider), and a save here has
      // an authorization waiting behind it that must now continue.
      await submitRequestedCredential(request, service, baseUrl, authType, authValue, envVarName);
    } else {
      await submitNewCredential(service, baseUrl, authType, authValue, envVarName);
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
            {/* A `secret` is sent nowhere, so it has no base URL. Hiding the
                field is what lets it stay `required` for every type that does
                need one. */}
            {!isSecret && (
            <div class="form-group">
              <label>Base URL</label>
              {/* Real state, not a bare `value=` with no handler. This input is
                  CONTROLLED, so every re-render rewrites it from its prop: it
                  only appeared to accept typing because nothing re-rendered.
                  Anything that does (picking an Auth Type, picking the base
                  provider a derived name runs on) silently reverted it, which
                  is how a derived connection kept the guessed
                  `https://<name>.com` instead of the registry row's base URL. */}
              <input
                ref={baseUrlRef}
                type="url"
                value={baseUrl}
                onInput={(e) => setBaseUrl((e.target as HTMLInputElement).value)}
                placeholder="e.g. https://api.github.com"
                required
              />
            </div>
            )}
            <div class="form-group">
              <label>Auth Type</label>
              <Dropdown
                options={[
                  { value: 'api_key', label: 'API Key' },
                  { value: 'bearer', label: 'Bearer Token' },
                  { value: 'basic', label: 'Basic Auth' },
                  { value: 'password', label: 'Password' },
                  { value: 'oauth_client', label: 'OAuth Client' },
                  {
                    value: 'secret',
                    label: 'Secret',
                    description: 'A shared secret, signed with rather than sent',
                  },
                ]}
                value={selectedAuthType}
                onChange={(v) => setSelectedAuthType(v as AuthType)}
              />
            </div>
            <div class="form-group">
              <FieldLabel
                title="Env var name"
                label={<>Env var name <span class="form-hint">(optional)</span></>}
              >
                  <p>
                    Also injects the secret under this name, in addition to the default{' '}
                    <code>CRED_&lt;NAME&gt;</code>.
                  </p>
              </FieldLabel>
              <input
                ref={envVarNameRef}
                type="text"
                defaultValue={initialEnvVarName}
                placeholder="MY_API_TOKEN"
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
            {repairNotice && <p class="credential-repair-notice">{repairNotice}</p>}
            {/* The one step this form cannot do for the user: the Client ID
                only exists once an app is registered with the provider, and the
                redirect URI has to be entered there character for character.
                Everything shown comes from the registry, so it stays true as
                providers change and the agent reads the same rows. */}
            {helpRow && (
              <div class="credential-console-help">
                {/* Shared with the two re-authorization surfaces, so the link
                    reads the same wherever the registry's console appears. */}
                <ConsoleLink row={helpRow} />
                {helpRow.setup_hint && <p>{helpRow.setup_hint}</p>}
                {helpRow.permissions_hint && <p>{helpRow.permissions_hint}</p>}
                {redirectUri && (
                  <div class="credential-redirect-uri">
                    <span class="label">Redirect URI</span>
                    <code>{redirectUri}</code>
                    <button
                      type="button"
                      class="action-btn"
                      onClick={() => void copyRedirectUri()}
                    >Copy</button>
                  </div>
                )}
              </div>
            )}
            {/* A name the registry does not know is a derived provider: a second,
                narrowly-scoped connection under its own name, running on a known
                provider's endpoints. Asking which one beats guessing from the
                spelling, and beats making the user paste four URLs they could
                have had for free. */}
            {!registryRow && !editing && knownProviders.length > 0 && (
              <div class="form-group">
                <FieldLabel
                  title="Runs on"
                  label={<>Runs on <span class="form-hint">(optional)</span></>}
                >
                    <p>
                      Pick the service this connection actually signs in to, and its
                      endpoints fill in below.
                    </p>
                    <p>
                      Use this for a separate, narrowly-scoped connection you want held
                      apart from your everyday one.
                    </p>
                </FieldLabel>
                <Dropdown
                  options={[
                    { value: '', label: 'A provider Lucidos does not know' },
                    ...knownProviders.map((p) => ({ value: p.id, label: p.label })),
                  ]}
                  value={basedOn}
                  onChange={applyBaseProvider}
                />
              </div>
            )}
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
              {/* Advisory, from the registry: the engine still derives the real
                  client type from whether a secret was saved. But "leave it
                  blank" is wrong advice for a provider that only issues
                  confidential clients, and both mismatches (a secret sent by a
                  public client, a secret-less redemption by a confidential one)
                  are refused by the provider late, looking nothing like their
                  cause. */}
              <FieldLabel
                title="Client Secret"
                label={
                  <>
                    Client Secret{' '}
                    <span class="form-hint">
                      {expectsSecret ? '(required by this provider)' : '(optional)'}
                    </span>
                  </>
                }
              >
                  <p>
                    {expectsSecret
                      ? "This provider's app registrations are confidential clients, so the "
                        + 'secret is part of the registration rather than an extra.'
                      : "Leave blank when the provider's app registration is a desktop or native app: "
                        + 'Lucidos then connects as a public client using PKCE instead of a secret.'}
                  </p>
              </FieldLabel>
              <SecretInput
                inputRef={clientSecretRef}
                defaultValue={initialFields.clientSecret}
                placeholder={
                  expectsSecret
                    ? 'Paste the secret generated with the client ID'
                    : 'Leave blank for a desktop/native app'
                }
              />
            </div>
            <details
              class="credential-oauth-endpoints"
              open={!hasPrefilledEndpoints || repairNamedEndpoints}
            >
              <summary>
                {hasPrefilledEndpoints
                  ? 'OAuth endpoints (pre-filled, edit if needed)'
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
                <FieldLabel title="Userinfo method">
                    <p>
                      Leave on GET unless the provider's userinfo endpoint refuses it.
                      Dropbox's does: <code>users/get_current_account</code> is
                      POST-only.
                    </p>
                    <p>
                      Getting this wrong costs only the account's name and email, never
                      the connection.
                    </p>
                </FieldLabel>
                <Dropdown
                  options={[
                    { value: 'GET', label: 'GET' },
                    { value: 'POST', label: 'POST' },
                  ]}
                  value={userinfoMethod}
                  onChange={setUserinfoMethod}
                />
              </div>
              <div class="form-group">
                <FieldLabel
                  title="Authorization parameters"
                  label={<>Authorization parameters <span class="form-hint">(optional)</span></>}
                >
                    <p>
                      Extra parameters added to the authorization URL, which is where a
                      provider spells "issue a refresh token" its own way.
                    </p>
                    <p>
                      Leave blank for <code>access_type=offline&amp;prompt=consent</code>.
                      Dropbox instead needs <code>token_access_type=offline</code>, and
                      without it its token expires in hours with nothing to renew it.
                    </p>
                    <p>
                      Write <code>none</code> to send neither.
                    </p>
                </FieldLabel>
                <input
                  ref={authorizeParamsRef}
                  type="text"
                  defaultValue={initialAuthorizeParams}
                  placeholder="access_type=offline&prompt=consent"
                />
              </div>
              <div class="form-group">
                <label>Default Scopes <span class="form-hint">(optional, space-separated)</span></label>
                <input ref={scopesRef} type="text" defaultValue={initialScopes} placeholder="read write user.read" />
              </div>
              <div class="form-group">
                <FieldLabel
                  title="Redirect URI"
                  label={<>Redirect URI <span class="form-hint">(optional)</span></>}
                >
                    <p>Register this exact URI with the provider.</p>
                    <p>
                      Leave blank for the default loopback-IP form; use{' '}
                      <code>http://localhost:14981/oauth/callback</code> only for a
                      provider that won't accept the IP literal.
                    </p>
                </FieldLabel>
                <input
                  ref={redirectUriRef}
                  type="url"
                  defaultValue={initialRedirectUri}
                  placeholder="http://127.0.0.1:14981/oauth/callback"
                />
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
