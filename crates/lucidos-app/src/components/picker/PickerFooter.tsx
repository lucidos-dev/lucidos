import { RemoteHeadersEditor } from '../settings/RemoteHeadersEditor';

import type { Ref, VNode } from 'preact';
import { Disclosure } from '../shared/Disclosure';
import { HiddenFileInput } from '../shared/HiddenFileInput';
import { ProgressBar } from '../shared/progressBar';
import { restoreButtonLabel, type CreateNote, type RestoreBlocker, type RestoreDraft, type RestoreUpload } from './workspaceForms';
import {
  AWAITING_OWNER_NOTE,
  HTTPS_ONLY_NOTE,
  TRUST_NOTE,
  remoteGatewayBlocker,
  shortNameInput,
  type RemoteGatewayDraft,
} from '../settings/remoteGatewayForm';

export type FooterMode = 'none' | 'create' | 'restore' | 'remote';

export interface PickerFooterProps {
  mode: FooterMode;
  onMode: (mode: FooterMode) => void;
  busy: boolean;
  restoreRunning: boolean;
  target: string | null;
  targetBlocker: string | null;

  name: string;
  onName: (name: string) => void;
  onCreate: () => void;
  onCancelCreate: () => void;
  suggestions: readonly string[];
  onSuggestion: (name: string) => void;
  nameInputRef?: Ref<HTMLInputElement>;
  createNote: CreateNote;

  draft: RestoreDraft;
  onDraft: (patch: Partial<RestoreDraft>) => void;
  onPickFile: (file: File | null | undefined) => void;
  restoreUpload: RestoreUpload;
  onRestore: () => void;
  onCancelRestore: () => void;
  blocker: RestoreBlocker | null;
  fileNote: string | null;
  onDeleteColliding: ((id: string) => void) | null;

  remoteDraft: RemoteGatewayDraft;
  onRemoteDraft: (patch: Partial<RemoteGatewayDraft>) => void;
  onAddRemote: () => void;
  onCancelRemote: () => void;
  pairedRemote: string | null;
  remoteAwaitingOwner: boolean;
}

export function pickerFooter(p: PickerFooterProps): VNode {
  const collision = p.blocker?.kind === 'collision' ? p.blocker : null;
  const targetOpen = p.targetBlocker === null;
  const canCreate = targetOpen && !p.busy && p.name.trim() !== '' && p.createNote?.blocking !== true;
  const canRestore = targetOpen && !p.busy && p.blocker === null;
  const remoteBlocker = remoteGatewayBlocker(p.remoteDraft);
  const canAddRemote = !p.busy && remoteBlocker === null;
  const remoteField = (label: string, key: 'address' | 'code' | 'name', placeholder: string) => (
    <label class="ws-picker-field">
      <span class="ws-picker-field-label">{label}</span>
      <input
        class="ws-picker-input"
        placeholder={placeholder}
        spellcheck={false}
        value={p.remoteDraft[key]}
        onInput={(e) => {
          const value = (e.target as HTMLInputElement).value;
          p.onRemoteDraft({ [key]: key === 'name' ? shortNameInput(value) : value });
        }}
        onKeyDown={(e) => e.key === 'Enter' && canAddRemote && p.onAddRemote()}
      />
    </label>
  );
  return (
    <footer class="ws-picker-footer">
      <div class="ws-picker-footer-actions">
        <button
          class={`ws-picker-new${p.mode === 'create' ? ' is-open' : ''}`}
          aria-expanded={p.mode === 'create'}
          disabled={!targetOpen}
          onClick={() => p.onMode('create')}
        >
          {p.target === null ? '+ New workspace' : `+ New workspace on ${p.target}`}
        </button>
        <button
          class={`ws-picker-new ws-picker-restore-open${p.mode === 'restore' ? ' is-open' : ''}`}
          aria-expanded={p.mode === 'restore'}
          disabled={p.restoreRunning || !targetOpen}
          onClick={() => p.onMode('restore')}
        >
          {p.target === null ? 'Restore from backup' : `Restore onto ${p.target}`}
        </button>
        <button
          class={`ws-picker-new${p.mode === 'remote' ? ' is-open' : ''}`}
          aria-expanded={p.mode === 'remote'}
          onClick={() => p.onMode('remote')}
        >
          Add a remote gateway
        </button>
      </div>
      <Disclosure open={!targetOpen}>
        <p class="ws-picker-note ws-picker-note-warn" role="status">{p.targetBlocker}</p>
      </Disclosure>

      <Disclosure open={p.mode === 'create'}>
        <div class="ws-picker-create">
          {!p.name.trim() && p.suggestions.length > 0 && (
            <div class="ws-picker-suggestions">
              <span class="ws-picker-suggestions-label">Try</span>
              {p.suggestions.map((s) => (
                <button
                  key={s}
                  type="button"
                  class="ws-picker-suggestion"
                  disabled={p.busy}
                  onClick={() => p.onSuggestion(s)}
                >
                  {s}
                </button>
              ))}
            </div>
          )}
          <div class="ws-picker-inline">
            <input
              ref={p.nameInputRef}
              class="ws-picker-input"
              placeholder="Workspace name"
              value={p.name}
              onInput={(e) => p.onName((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => e.key === 'Enter' && canCreate && p.onCreate()}
              onFocus={(e) => (e.target as HTMLInputElement).select()}
              autoFocus
            />
            <button
              class="ws-picker-btn ws-picker-btn-confirm"
              disabled={!canCreate}
              onClick={p.onCreate}
            >
              {p.busy ? 'Creating…' : 'Create'}
            </button>
            <button class="ws-picker-btn" onClick={p.onCancelCreate}>Cancel</button>
          </div>
          {p.createNote && (
            <p class={`ws-picker-note${p.createNote.blocking ? ' ws-picker-note-warn' : ''}`}>
              {p.createNote.message}
            </p>
          )}
        </div>
      </Disclosure>

      <Disclosure open={p.mode === 'restore'}>
        <div class="ws-picker-restore-form">
          <p class="ws-picker-lead">
            Restore a workspace{p.target === null ? '' : ` onto ${p.target}`} from an encrypted backup. You need the <code>.age</code> or{' '}
            <code>.enc</code> file and the backup key you saved when you set backups up.
          </p>

          <div class="ws-picker-field">
            <span class="ws-picker-field-label">Backup file</span>
            <label
              class="ws-picker-restore-drop"
              data-picked={p.draft.file ? 'true' : 'false'}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key !== 'Enter' && e.key !== ' ') return;
                e.preventDefault();
                (e.currentTarget as HTMLElement).querySelector('input')?.click();
              }}
              onDragOver={(e) => e.preventDefault()}
              onDrop={(e) => {
                e.preventDefault();
                p.onPickFile(e.dataTransfer?.files?.[0]);
              }}
            >
              <HiddenFileInput
                onChange={(e) => {
                  const input = e.target as HTMLInputElement;
                  p.onPickFile(input.files?.[0]);
                  input.value = '';
                }}
              />
              <span>{p.draft.file ? p.draft.file.name : 'Drop a .age or .enc backup here, or click to choose'}</span>
            </label>
          </div>

          <label class="ws-picker-field">
            <span class="ws-picker-field-label">Backup key</span>
            <input
              class="ws-picker-input"
              type="password"
              placeholder="Paste the key you saved"
              value={p.draft.key}
              onInput={(e) => p.onDraft({ key: (e.target as HTMLInputElement).value })}
            />
          </label>

          <label class="ws-picker-field">
            <span class="ws-picker-field-label">Workspace name</span>
            <input
              class="ws-picker-input"
              placeholder="Name for the restored workspace"
              value={p.draft.name}
              onInput={(e) => p.onDraft({ name: (e.target as HTMLInputElement).value })}
              onKeyDown={(e) => e.key === 'Enter' && canRestore && p.onRestore()}
            />
          </label>

          {p.fileNote && <p class="ws-picker-note ws-picker-note-warn">{p.fileNote}</p>}

          {collision ? (
            <div class="ws-picker-restore-warn">
              <span>{collision.message}</span>
              {p.onDeleteColliding && (
                <button
                  class="ws-picker-btn ws-picker-btn-danger"
                  disabled={p.busy}
                  onClick={() => p.onDeleteColliding?.(collision.existing.id)}
                >
                  Delete “{collision.existing.name}”…
                </button>
              )}
            </div>
          ) : (
            p.blocker && <p class="ws-picker-note">{p.blocker.message}</p>
          )}

          {(p.restoreUpload.kind === 'uploading' || p.restoreUpload.kind === 'retrying') && (
            <ProgressBar done={p.restoreUpload.sentBytes} total={p.restoreUpload.totalBytes} />
          )}
          {p.restoreUpload.kind === 'retrying' && (
            <p class="ws-picker-note ws-picker-note-warn">{p.restoreUpload.reason}. Retrying.</p>
          )}

          <div class="ws-picker-inline">
            <button
              class="ws-picker-btn ws-picker-btn-confirm"
              disabled={!canRestore}
              onClick={p.onRestore}
            >
              {restoreButtonLabel(p.restoreUpload)}
            </button>
            <button
              class="ws-picker-btn"
              disabled={p.restoreUpload.kind === 'starting'}
              onClick={p.onCancelRestore}
            >
              {p.restoreUpload.kind === 'idle' ? 'Cancel' : 'Cancel upload'}
            </button>
          </div>
        </div>
      </Disclosure>

      <Disclosure open={p.mode === 'remote'}>
        <div class="ws-picker-restore-form">
          <p class="ws-picker-lead">
            Open another Lucidos gateway's workspaces from here. On that gateway, open
            Settings → Access → Pair a gateway for a code and its https address.
          </p>
          <p class="ws-picker-note ws-picker-note-warn">{TRUST_NOTE}</p>
          {remoteField('Short name', 'name', 'cloud')}
          {remoteField('Gateway pairing secret', 'code', 'lgp_…')}
          <RemoteHeadersEditor rows={p.remoteDraft.headers ?? []} onChange={headers => p.onRemoteDraft({ headers })} disabled={p.busy} />
          {remoteField('Address', 'address', 'https://lucidos.example.com')}
          {remoteBlocker?.kind === 'plain-http' && (
            <p class="ws-picker-note ws-picker-note-warn">{HTTPS_ONLY_NOTE}</p>
          )}
          {p.pairedRemote && (
            <p class="ws-picker-note">Paired with {p.pairedRemote}.</p>
          )}
          <Disclosure open={p.remoteAwaitingOwner}>
            <p class="ws-picker-note" role="status">{AWAITING_OWNER_NOTE}</p>
          </Disclosure>
          <div class="ws-picker-inline">
            <button
              class="ws-picker-btn ws-picker-btn-confirm"
              disabled={!canAddRemote}
              onClick={p.onAddRemote}
            >
              {p.busy ? 'Pairing…' : 'Pair'}
            </button>
            <button class="ws-picker-btn" onClick={p.onCancelRemote}>Cancel</button>
          </div>
        </div>
      </Disclosure>
    </footer>
  );
}
