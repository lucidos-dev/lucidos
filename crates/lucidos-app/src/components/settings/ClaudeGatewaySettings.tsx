import { useEffect, useState } from 'preact/hooks';
import { preferences } from '../../store/store';
import { currentClaudeGatewayUrl } from '../../store/actions/preferences';
import {
  claudeGatewayStatus,
  loadClaudeGatewayStatus,
  submitClaudeGatewaySignIn,
  submitClaudeGatewaySignOut,
} from '../../store/actions/claudeGateway';
import { openUrlOutsideApp } from '../../store/actions/artifacts';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { LoadableError } from '../shared/LoadableError';
import { ProviderBlock } from './ProviderBlock';
import { claudeGatewayView, initialGatewayUrl, type ClaudeGatewayView } from './claudeGatewayView';

/** Configure the Claude gateway provider (Settings → Models → Providers): the
 *  Anthropic API behind an organisation's own sign-in. Lucidos signs in as
 *  itself with a device code, and the engine keeps and refreshes the tokens.
 *
 *  The URL field starts on the stored `claude_gateway_url`, else on the gateway
 *  Claude Code's managed settings name. Nothing is stored until Sign in, which
 *  sends the URL with the request.
 *
 *  Renders only the provider block; `SettingsView` owns the section. */
export function ClaudeGatewaySettings() {
  preferences.value;
  const loadable = claudeGatewayStatus.value;
  useEffect(() => {
    if (claudeGatewayStatus.value.status === 'not-loaded') void loadClaudeGatewayStatus();
  }, []);

  const status = loadable.status === 'loaded' ? loadable.data : null;
  const view = status && claudeGatewayView(status);
  const account = view?.kind === 'signed-in' ? view.account : null;

  return (
    <ProviderBlock
      id="claude-gateway"
      label="Claude gateway"
      anchor="models:claude-gateway"
      hasStoredConfig={!!account}
      explainer={
        <>
          <p>
            Serves Claude models on the <strong>claude-gateway</strong> provider,
            through your organisation's Claude gateway: the Anthropic API behind
            your organisation's sign-in.
          </p>
          <p>
            Lucidos signs in as itself, apart from Claude Code. The gateway decides
            which models your account may use, and a model it does not list is not
            served here.
          </p>
          <p>
            Also reachable as <code>lucidos claude-gateway sign-in</code>. No launch
            environment variable sets it up, since signing in needs you.
          </p>
        </>
      }
      detail={account && (
        <span class="list-row-details">
          {account.email ? `signed in as ${account.email}` : 'signed in'}
        </span>
      )}
      actions={account && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void submitClaudeGatewaySignOut()}
        >
          Sign out
        </button>
      )}
    >
      {loadable.status === 'failed' ? (
        <LoadableError
          noun="Claude gateway status"
          error={loadable.error}
          onRetry={() => void loadClaudeGatewayStatus()}
        />
      ) : (
        <GatewayRows view={view} suggestedUrl={status?.suggested_url ?? null} />
      )}
    </ProviderBlock>
  );
}

/** The rows under the switch. Before the status loads they draw the
 *  signed-out form with Sign in disabled, so nothing moves when it lands. */
function GatewayRows({ view, suggestedUrl }: {
  view: ClaudeGatewayView | null;
  suggestedUrl: string | null;
}) {
  if (view?.kind === 'signed-in') {
    return (
      <>
        <div class="settings-row">
          <span class="settings-row-label">Gateway</span>
          <span>{view.account.issuer}</span>
        </div>
        {view.telemetry && (
          <p class="settings-row-note">
            Your organisation's gateway collects usage telemetry for your account:
            model, tokens, cost and timing per thread, never content.
          </p>
        )}
      </>
    );
  }
  if (view?.kind === 'pending') return <PendingRows view={view} />;
  return <SignInRows notice={view?.notice ?? null} loaded={view !== null} suggestedUrl={suggestedUrl} />;
}

function SignInRows({ notice, loaded, suggestedUrl }: {
  notice: Extract<ClaudeGatewayView, { kind: 'signed-out' }>['notice'];
  loaded: boolean;
  suggestedUrl: string | null;
}) {
  const stored = currentClaudeGatewayUrl();
  const [url, setUrl] = useServerBackedField(initialGatewayUrl(stored, suggestedUrl));
  const [starting, setStarting] = useState(false);
  const fromManagedSettings = !stored && !!suggestedUrl && url.trim() === suggestedUrl;

  async function signIn() {
    setStarting(true);
    try {
      await submitClaudeGatewaySignIn(url.trim());
    } finally {
      setStarting(false);
    }
  }

  return (
    <>
      {notice && (
        <p class={notice.kind === 'failed' ? 'settings-field-error' : 'settings-row-note'}>
          {notice.kind === 'failed'
            ? `The last sign-in failed: ${notice.error}`
            : 'The gateway ended your sign-in. Sign in again to keep using it.'}
        </p>
      )}
      <div class="settings-row">
        <span class="settings-row-label">Gateway URL</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder="https://gateway.example"
          value={url}
          onInput={(e) => setUrl((e.target as HTMLInputElement).value)}
        />
      </div>
      {fromManagedSettings && (
        <p class="settings-row-note">From Claude Code's managed settings on this computer.</p>
      )}
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={!loaded || starting || !url.trim()}
          onClick={() => void signIn()}
        >
          {notice ? 'Sign in again' : 'Sign in'}
        </button>
      </div>
    </>
  );
}

function PendingRows({ view }: { view: Extract<ClaudeGatewayView, { kind: 'pending' }> }) {
  const expires = new Date(view.expiresAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  return (
    <>
      <p class="settings-row-note">
        Open the sign-in page and confirm this code. It works until {expires}.
      </p>
      <div class="settings-row">
        <span class="settings-row-label">Code</span>
        <code class="claude-gateway-code">{view.userCode}</code>
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        {/* Stopping is a sign-out, which would also forget a stored sign-in. */}
        {!view.signedIn && (
          <button
            class="action-btn action-btn-secondary"
            onClick={() => void submitClaudeGatewaySignOut()}
          >
            Cancel
          </button>
        )}
        <button class="action-btn" onClick={() => openUrlOutsideApp(view.signInPage)}>
          Open sign-in page
        </button>
      </div>
    </>
  );
}
