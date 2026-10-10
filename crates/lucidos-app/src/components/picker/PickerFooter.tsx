/**
 * The picker's footer: the two permanent entry points ("New workspace" /
 * "Restore from backup") and whichever form is unfolded beneath them.
 *
 * A pure vnode builder taking its whole state as props (same shape as
 * `networkAccessBody` next door), so what the user is shown in each state is
 * unit-testable without a DOM, and `WorkspacePicker.tsx` keeps only the wiring.
 *
 * The load-bearing rule here: **both entry points render in every state.** They
 * are how the user moves between creating and restoring, so a mode that renders
 * one INSTEAD of the other strands them. With zero workspaces the create form
 * used to replace the footer wholesale, which hid restore in precisely the
 * situation it exists for (ADR 0015: a user with a backup and no workspace).
 */

import type { Ref, VNode } from 'preact';
import { HiddenFileInput } from '../shared/HiddenFileInput';
import type { CreateNote, RestoreBlocker, RestoreDraft } from './workspaceForms';

/** Which form is unfolded under the entry points. */
export type FooterMode = 'none' | 'create' | 'restore';

export interface PickerFooterProps {
  mode: FooterMode;
  /** Activate an entry point. The caller decides that re-activating the open one
   *  closes it, so the toggle rule lives with the state it owns. */
  onMode: (mode: FooterMode) => void;
  busy: boolean;
  /** A restore is already running: the gateway holds one slot, so the entry
   *  point says so instead of opening a form that cannot submit. */
  restoreRunning: boolean;

  // Create form.
  name: string;
  onName: (name: string) => void;
  onCreate: () => void;
  onCancelCreate: () => void;
  /** Quick-fill chips, shown while the name is empty. Empty once the user has a
   *  workspace, and the whole row (its "Try" label included) goes with them. */
  suggestions: readonly string[];
  onSuggestion: (name: string) => void;
  nameInputRef?: Ref<HTMLInputElement>;
  /** What the create form has to say about the typed name: a blocking duplicate,
   *  a non-blocking note about the address it will get, or nothing. */
  createNote: CreateNote;

  // Restore form.
  draft: RestoreDraft;
  onDraft: (patch: Partial<RestoreDraft>) => void;
  onPickFile: (file: File | null | undefined) => void;
  onRestore: () => void;
  onCancelRestore: () => void;
  /** Why Restore is disabled, or null when it can start. */
  blocker: RestoreBlocker | null;
  /** Note about a chosen file that doesn't look like an archive. */
  fileNote: string | null;
  /** Offer to remove the workspace holding the address the user asked for. */
  onDeleteColliding: (id: string) => void;
}

export function pickerFooter(p: PickerFooterProps): VNode {
  // Narrowed once so the collision branch can reach the workspace in the way
  // without re-asserting it inside a handler.
  const collision = p.blocker?.kind === 'collision' ? p.blocker : null;
  // ONE condition per form, shared by the button's disabled state and the
  // Enter-key path. Two copies drift: Enter used to submit a duplicate name the
  // disabled Create button was already refusing, and the only thing that caught
  // it was the gateway answering 409.
  const canCreate = !p.busy && p.name.trim() !== '' && p.createNote?.blocking !== true;
  const canRestore = !p.busy && p.blocker === null;
  return (
    <footer class="ws-picker-footer">
      <div class="ws-picker-footer-actions">
        <button
          class={`ws-picker-new${p.mode === 'create' ? ' is-open' : ''}`}
          aria-pressed={p.mode === 'create'}
          onClick={() => p.onMode('create')}
        >
          + New workspace
        </button>
        <button
          class={`ws-picker-new ws-picker-restore-open${p.mode === 'restore' ? ' is-open' : ''}`}
          aria-pressed={p.mode === 'restore'}
          disabled={p.restoreRunning}
          onClick={() => p.onMode('restore')}
        >
          Restore from backup
        </button>
      </div>

      {p.mode === 'create' && (
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
          {/* Either "that name is taken" (which blocks) or "the address it wants
              is taken, so it will live at /x-2/" (which does not). Both are
              things the user would otherwise only discover afterwards. */}
          {p.createNote && (
            <p class={`ws-picker-note${p.createNote.blocking ? ' ws-picker-note-warn' : ''}`}>
              {p.createNote.message}
            </p>
          )}
        </div>
      )}

      {p.mode === 'restore' && (
        <div class="ws-picker-restore-form">
          <p class="ws-picker-lead">
            Restore a workspace from an encrypted backup. You need the <code>.enc</code> file and
            the backup key you saved when you set backups up.
          </p>

          <div class="ws-picker-field">
            <span class="ws-picker-field-label">Backup file</span>
            <label
              class="ws-picker-restore-drop"
              data-picked={p.draft.file ? 'true' : 'false'}
              // The label is the whole hit target and the input inside it is
              // off-screen, so give keyboard users the same door: the input
              // itself is aria-hidden and untabbable by design.
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
              {/* `HiddenFileInput`, not a bare `hidden` input: `display: none`
                  drops the `change` event on iOS in standalone PWA mode once the
                  file picker dismisses, so the pick silently never arrives and
                  the form sits there looking half-filled. The shared component
                  hides the input off-screen but IN layout, which is what makes
                  the event land, and it must stay inside this label so the tap
                  reaches it as one native gesture.

                  No `accept`: `.enc` has no registered UTI, so an accept filter
                  greys out every file in the iOS/iPadOS Files picker. The gateway
                  validates the archive, and a non-.enc pick is called out below
                  without being blocked. */}
              <HiddenFileInput
                onChange={(e) => {
                  const input = e.target as HTMLInputElement;
                  p.onPickFile(input.files?.[0]);
                  // Let the same file be re-picked later: an unchanged value
                  // fires no further `change`.
                  input.value = '';
                }}
              />
              <span>{p.draft.file ? p.draft.file.name : 'Drop a .enc backup here, or click to choose'}</span>
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

          {/* Whatever is missing, say it. A dead Restore button with nothing next
              to it is what sent two people hunting for the cause in the wrong
              place. The collision case names the workspace the way the list shows
              it, and offers to remove that one. */}
          {collision ? (
            <div class="ws-picker-restore-warn">
              <span>{collision.message}</span>
              <button
                class="ws-picker-btn ws-picker-btn-danger"
                disabled={p.busy}
                onClick={() => p.onDeleteColliding(collision.existing.id)}
              >
                Delete “{collision.existing.name}”…
              </button>
            </div>
          ) : (
            p.blocker && <p class="ws-picker-note">{p.blocker.message}</p>
          )}

          <div class="ws-picker-inline">
            <button
              class="ws-picker-btn ws-picker-btn-confirm"
              disabled={!canRestore}
              onClick={p.onRestore}
            >
              {p.busy ? 'Starting…' : 'Restore'}
            </button>
            <button class="ws-picker-btn" onClick={p.onCancelRestore}>Cancel</button>
          </div>
        </div>
      )}
    </footer>
  );
}
