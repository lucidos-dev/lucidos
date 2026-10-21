import { type VNode } from 'preact';
import { useState } from 'preact/hooks';
import { showToast } from '../../store/store';
import { copyToClipboard } from '../../utils/clipboard';
import { errorDetail } from '../../utils/errorDetail';
import { formatDurationPhrase } from '../../utils/formatTime';
import { Disclosure } from '../shared/Disclosure';
import { Explainer } from '../shared/Explainer';
import { SkeletonProvider, SkBlock, SkText } from '../shared/Skeleton';
import {
  confirmBackupKey,
  getBackupKey,
  setOwnBackupRecipient,
  startBackupKeySetup,
  verifyBackupKey,
  type BackupKeys,
} from '../../api/client';

/** Where the user is in a key flow. Each step holds only what it shows, so a
 *  backup key minted for one setup cannot leak into another step. */
export type KeyStep =
  | { kind: 'idle' }
  | { kind: 'legacy'; legacyKey: string }
  | { kind: 'show-new'; backupKey: string }
  | { kind: 'paste' }
  | { kind: 'verify' }
  | { kind: 'own' };

const PENDING_LINE = 'A new backup key is waiting for you to paste it back.';

/** What seals backups now, in one line. A pending switch away from a legacy key
 *  or from no key names only the switch, since that is the next step. */
export function backupKeyStatusLine(keys: BackupKeys): string {
  if (keys.recipient) {
    const checked = keys.secs_since_verified == null
      ? 'You have not checked your backup key yet.'
      : `You last checked your backup key ${formatDurationPhrase(keys.secs_since_verified)} ago.`;
    const status = `Backups are sealed to your backup key. ${checked}`;
    return keys.pending ? `${status} ${PENDING_LINE}` : status;
  }
  if (keys.pending) return PENDING_LINE;
  if (keys.legacy_key) {
    return 'This workspace still uses an old backup key. Switch to a new backup key that only you hold.';
  }
  return 'Backups are not set up. Create a backup key first. Only you hold it, so nobody else can open your backups.';
}

/** Whether anything can seal a backup right now. */
export function backupsCanSeal(keys: BackupKeys): boolean {
  return keys.recipient !== null || keys.legacy_key;
}

/** The panel's label, with what age is behind the icon. */
function backupKeyTitle(): VNode {
  return (
    <span class="settings-row-label">
      Backup key
      <Explainer title="Backup key">
        <p>
          Lucidos encrypts backups with{' '}
          <a class="accent-link" href="https://age-encryption.org" target="_blank" rel="noopener noreferrer">
            age
          </a>
          , an open-source file encryption tool.
        </p>
        <p>
          Your backup key is an age secret key. It starts with <code>AGE-SECRET-KEY-1</code>. Once you set one
          up, Lucidos keeps only its public half, so it can seal backups but never open them. To restore, you
          need the key you saved.
        </p>
        <p>
          Already use age? When you set up or switch keys, pick <strong>Use my own age public key</strong> and
          paste a public key that starts with <code>age1</code>.
        </p>
      </Explainer>
    </span>
  );
}

/** The key block as shimmer: its label, status line and one row of buttons. */
export function backupKeyPanelSkeleton(): VNode {
  return (
    <SkeletonProvider>
      <div class="backup-key" aria-hidden="true">
        <span class="settings-row-label">Backup key</span>
        <SkText as="div" class="backup-key-status" w="70%" />
        <div class="backup-key-actions">
          <SkBlock w="8rem" h="1.75rem" />
          <SkBlock w="11rem" h="1.75rem" />
        </div>
      </div>
    </SkeletonProvider>
  );
}

/**
 * The backup key: setup with the paste-back gate, verify, rotate, the user's
 * own age public key, and the switch from a legacy key.
 *
 * The engine never stores a backup key. It shows a minted one once, and
 * backups keep their current key until the user pastes the new one back.
 */
export function BackupKeyPanel({
  keys,
  onKeysChanged,
}: {
  keys: BackupKeys;
  onKeysChanged: (keys: BackupKeys) => void;
}) {
  const [step, setStep] = useState<KeyStep>({ kind: 'idle' });
  const [pasted, setPasted] = useState('');
  const [busy, setBusy] = useState(false);

  function go(next: KeyStep) {
    setPasted('');
    setStep(next);
  }

  async function run(action: () => Promise<void>, failure: string) {
    setBusy(true);
    try {
      await action();
    } catch (err) {
      showToast(`${failure}: ${errorDetail(err)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  const startSetup = () => void run(async () => {
    go({ kind: 'show-new', backupKey: await startBackupKeySetup() });
  }, 'Failed to create a backup key');

  const revealLegacy = () => void run(async () => {
    go({ kind: 'legacy', legacyKey: (await getBackupKey()).key });
  }, 'Failed to show the old backup key');

  const confirm = () => void run(async () => {
    onKeysChanged(await confirmBackupKey(pasted.trim()));
    go({ kind: 'idle' });
    showToast('Backups now use your new backup key', 'success');
  }, 'Backup key not confirmed');

  const verify = () => void run(async () => {
    if (await verifyBackupKey(pasted.trim())) {
      go({ kind: 'idle' });
      showToast('That backup key opens your backups', 'success');
    } else {
      showToast('That backup key does not open your backups. Look for the one you saved, or rotate to a new one.', 'error');
    }
  }, 'Failed to check the backup key');

  const useOwn = () => void run(async () => {
    onKeysChanged(await setOwnBackupRecipient(pasted.trim()));
    go({ kind: 'idle' });
    showToast('Backups now use your own age key', 'success');
  }, 'Failed to use that age public key');

  const cancel = (
    <button class="action-btn action-btn-secondary" disabled={busy} onClick={() => go({ kind: 'idle' })}>
      Cancel
    </button>
  );

  /** Leads its row, apart from the step's own buttons on the right. */
  function copyButton(key: string, copied: string) {
    return (
      <button class="action-btn backup-key-copy" onClick={() => copyToClipboard(key, copied)}>
        Copy
      </button>
    );
  }

  function pasteField(placeholder: string) {
    return (
      <input
        type="text"
        class="settings-text-input backup-key-input"
        spellcheck={false}
        placeholder={placeholder}
        value={pasted}
        onInput={(e) => setPasted((e.target as HTMLInputElement).value)}
      />
    );
  }

  function stepBody(): VNode | null {
    switch (step.kind) {
      case 'idle':
        return null;
      case 'legacy':
        return (
          <>
            <p class="backup-key-note">
              This is your old backup key. Your old <code>.enc</code> backups still need it, and after the
              switch Lucidos forgets it. Save it now if you have not already.
            </p>
            <div class="backup-key-reveal">
              <span class="backup-key-value">{step.legacyKey}</span>
            </div>
            <div class="backup-key-actions">
              {copyButton(step.legacyKey, 'Old backup key copied')}
              {cancel}
              <button class="action-btn" disabled={busy} onClick={() => go({ kind: 'own' })}>
                Use my own age public key
              </button>
              <button class="action-btn action-btn-confirm" disabled={busy} onClick={startSetup}>
                Create a new backup key
              </button>
            </div>
          </>
        );
      case 'show-new':
        return (
          <>
            <p class="backup-key-warning">
              Save this backup key somewhere safe, such as your password manager. Lucidos shows it only now
              and cannot recover it. Without it, nobody can restore your backups.
            </p>
            <div class="backup-key-reveal">
              <span class="backup-key-value">{step.backupKey}</span>
            </div>
            {keys.recipient && (
              <p class="backup-key-note">Your older backups still need your previous backup key.</p>
            )}
            <div class="backup-key-actions">
              {copyButton(step.backupKey, 'Backup key copied')}
              {cancel}
              <button class="action-btn action-btn-confirm" onClick={() => go({ kind: 'paste' })}>
                I saved it
              </button>
            </div>
          </>
        );
      case 'paste':
        return (
          <>
            <p class="backup-key-note">
              Paste the backup key you saved. Backups start using it once it matches.
            </p>
            {pasteField('AGE-SECRET-KEY-1…')}
            <div class="backup-key-actions">
              {cancel}
              <button class="action-btn action-btn-confirm" disabled={busy || !pasted.trim()} onClick={confirm}>
                Confirm backup key
              </button>
            </div>
          </>
        );
      case 'verify':
        return (
          <>
            <p class="backup-key-note">
              Paste your backup key to check that it still opens your backups. Lucidos compares it and forgets it.
            </p>
            {pasteField('AGE-SECRET-KEY-1…')}
            <div class="backup-key-actions">
              {cancel}
              <button class="action-btn action-btn-confirm" disabled={busy || !pasted.trim()} onClick={verify}>
                Check backup key
              </button>
            </div>
          </>
        );
      case 'own':
        return (
          <>
            <p class="backup-key-note">
              Paste the public key of an age identity you already hold. New backups are sealed to it, and only
              your matching private key opens them.
            </p>
            {pasteField('age1…')}
            <div class="backup-key-actions">
              {cancel}
              <button class="action-btn action-btn-confirm" disabled={busy || !pasted.trim()} onClick={useOwn}>
                Use this key
              </button>
            </div>
          </>
        );
    }
  }

  function idleActions(): VNode {
    if (keys.pending) {
      return (
        <>
          <button class="action-btn" disabled={busy} onClick={startSetup}>Start over</button>
          <button class="action-btn action-btn-confirm" disabled={busy} onClick={() => go({ kind: 'paste' })}>
            Paste backup key
          </button>
        </>
      );
    }
    if (keys.recipient) {
      return (
        <>
          <button class="action-btn" disabled={busy} onClick={() => go({ kind: 'own' })}>
            Use my own age public key
          </button>
          <button class="action-btn" disabled={busy} onClick={startSetup}>Rotate</button>
          <button class="action-btn action-btn-confirm" disabled={busy} onClick={() => go({ kind: 'verify' })}>
            Verify backup key
          </button>
        </>
      );
    }
    if (keys.legacy_key) {
      return (
        <button class="action-btn action-btn-confirm" disabled={busy} onClick={revealLegacy}>
          Switch to a new backup key
        </button>
      );
    }
    return (
      <>
        <button class="action-btn" disabled={busy} onClick={() => go({ kind: 'own' })}>
          Use my own age public key
        </button>
        <button class="action-btn action-btn-confirm" disabled={busy} onClick={startSetup}>
          Create backup key
        </button>
      </>
    );
  }

  const body = stepBody();
  return (
    <div class="backup-key" data-search-anchor="backup:key">
      {backupKeyTitle()}
      {/* A step says what is happening itself, so the status line steps aside. */}
      <Disclosure open={step.kind === 'idle'} bodyClass="backup-key-step">
        <div class="backup-key-status" data-state={backupsCanSeal(keys) && !keys.legacy_key ? 'ok' : 'warn'}>
          {backupKeyStatusLine(keys)}
        </div>
        <div class="backup-key-actions">{idleActions()}</div>
      </Disclosure>
      <Disclosure open={body !== null} bodyClass="backup-key-step">
        {body}
      </Disclosure>
    </div>
  );
}
