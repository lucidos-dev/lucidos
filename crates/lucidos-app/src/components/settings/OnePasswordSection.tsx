import { useEffect, useRef, useState } from 'preact/hooks';
import { credentials, showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import {
  deleteOnepassword,
  getOnepasswordStatus,
  saveOnepassword,
  testOnepassword,
  type OnepasswordStatus,
  type OnepasswordVaultCheck,
} from '../../api/client/settings';
import { errorDetail } from '../../utils/errorDetail';
import { Explainer } from '../shared/Explainer';
import { FormSkeleton } from '../shared/FormSkeleton';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SecretInput } from '../shared/SecretInput';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useServerBackedField } from '../../hooks/useServerBackedField';

const ONEPASSWORD_FIELDS = [{ label: 'Service account token' }, { label: 'op path' }];

/** What a passing Test says: the vaults the service account reaches. */
export function describeVaultCheck(check: OnepasswordVaultCheck): string {
  if (check.vaults.length === 0) return `${check.cli} works, but this service account reaches no vaults.`;
  const noun = check.vaults.length === 1 ? 'vault' : 'vaults';
  return `Reaches ${check.vaults.length} ${noun}: ${check.vaults.join(', ')}.`;
}

/** Settings, 1Password: the service account token that reads every *secret
 *  reference*, where `op` lives, and a Test that runs it.
 *
 *  The token never comes back from the engine, so the field starts blank and
 *  a blank save keeps the stored token. Re-reads whenever the credentials
 *  list changes, since every save or delete moves it. */
export function OnePasswordSection() {
  const [status, setStatus] = useState<Loadable<OnepasswordStatus>>({ status: 'not-loaded' });
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const [check, setCheck] = useState<string | null>(null);
  const tokenRef = useRef<HTMLInputElement>(null);
  const credentialList = credentials.value;

  useEffect(() => {
    let cancelled = false;
    setStatus((current) => (current.status === 'loaded' ? current : { status: 'loading' }));
    void getOnepasswordStatus().then(
      (data) => !cancelled && setStatus({ status: 'loaded', data }),
      (e) => !cancelled && setStatus(toFailed(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [credentialList, attempt]);

  const saved = status.status === 'loaded' && status.data.saved;
  const [cliPath, setCliPath] = useServerBackedField(
    status.status === 'loaded' ? status.data.cli_path ?? '' : '',
  );
  const showLoading = useDelayedLoading(status);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    try {
      await action();
    } catch (e) {
      showToast(errorDetail(e), 'error', { title: '1Password' });
    } finally {
      setBusy(false);
    }
  }

  const save = () =>
    void run(async () => {
      await saveOnepassword(tokenRef.current?.value ?? '', cliPath);
      if (tokenRef.current) tokenRef.current.value = '';
      setCheck(null);
      showToast('1Password service account saved', 'success');
    });

  const test = () =>
    void run(async () => {
      setCheck(null);
      const result = await testOnepassword();
      setCheck(describeVaultCheck(result));
    });

  const remove = () =>
    void run(async () => {
      const confirmed = await showConfirm(
        'Remove the 1Password service account?\n\nEvery credential that holds a 1Password reference stops working until you add one again.',
        'Remove',
        { variant: 'danger' },
      );
      if (!confirmed) return;
      await deleteOnepassword();
      setCheck(null);
    });

  return (
    <div class="settings-section accounts-section">
      <div class="settings-section-title" data-search-anchor="accounts:1password">
        1Password
        <Explainer title="1Password">
          <p>
            A credential can hold a 1Password reference, such as{' '}
            <code>op://vault/item/field</code>, instead of the secret. Lucidos reads it
            from 1Password each time it is used and never stores the value.
          </p>
          <p>
            It reads through the 1Password command line tool, <code>op</code>, signed in
            with a service account. Make one in 1Password with read access to the vaults
            you need, and paste its token here.
          </p>
        </Explainer>
      </div>
      {status.status === 'failed' ? (
        <LoadableError
          noun="1Password settings"
          error={status.error}
          onRetry={() => setAttempt((n) => n + 1)}
        />
      ) : (
        <LoadingFade showSkeleton={showLoading} skeleton={<FormSkeleton fields={ONEPASSWORD_FIELDS} />}>
          {status.status === 'loaded' && (
            <div class="onepassword-form">
              <div class="form-group">
                <label>Service account token</label>
                <SecretInput
                  inputRef={tokenRef}
                  placeholder={saved ? 'Saved. Paste a new token to replace it' : 'ops_…'}
                />
              </div>
              <div class="form-group">
                <label>
                  op path <span class="form-hint">(optional)</span>
                </label>
                <input
                  type="text"
                  value={cliPath}
                  onInput={(e) => setCliPath((e.currentTarget as HTMLInputElement).value)}
                  placeholder="Found on the PATH, for example /opt/homebrew/bin/op"
                  spellcheck={false}
                />
              </div>
              {check && <p class="onepassword-check">{check}</p>}
              <div class="form-actions">
                {saved && (
                  <button type="button" class="action-btn action-btn-danger" disabled={busy} onClick={remove}>
                    Remove
                  </button>
                )}
                {saved && (
                  <button type="button" class="action-btn" disabled={busy} onClick={test}>
                    Test
                  </button>
                )}
                <button type="button" class="action-btn action-btn-confirm" disabled={busy} onClick={save}>
                  Save
                </button>
              </div>
            </div>
          )}
        </LoadingFade>
      )}
    </div>
  );
}
