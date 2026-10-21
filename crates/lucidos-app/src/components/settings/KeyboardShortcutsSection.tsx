import { useState, useEffect } from 'preact/hooks';
import { PREF_COMPOSER_ENTER_KEY } from '@lucidos/preference-catalog';
import { SHORTCUT_DEFS, formatBinding, shortcutDef, shortcutSearchAnchor, type Binding, type ShortcutId } from '../../utils/shortcuts';
import { displayBinding, isCustomized, setBinding, resetBinding, recordChord } from '../../store/actions/keybindings';
import { currentComposerEnterKey, setComposerEnterKey, type ComposerEnterKey } from '../../store/actions/preferences';
import { isMac } from '../../utils/platform';
import { Explainer } from '../shared/Explainer';

// Derived, not hand-maintained: a category added to the registry can't be
// silently missing from the sheet. Order = first appearance in SHORTCUT_DEFS.
const CATEGORIES = [...new Set(SHORTCUT_DEFS.map((d) => d.category))];

export const COMPOSER_ENTER_KEY_LABELS: Record<ComposerEnterKey, string> = {
  send: 'Sends',
  newline: 'New line',
};

const enterChord = (mod: boolean, shift: boolean): string =>
  formatBinding({ mod, shift, alt: false, key: 'Enter' } satisfies Binding, isMac);

/** The Composer group: what Enter does in the chat composer. */
function ComposerEnterKeyRow() {
  const mode = currentComposerEnterKey();
  const enter = enterChord(false, false);
  const shiftEnter = enterChord(false, true);
  const modEnter = enterChord(true, false);
  return (
    <div class="settings-section">
      <div class="settings-section-title">Composer</div>
      <div class="settings-row" data-search-anchor="keyboard-shortcuts:composer-enter-key">
        <span class="settings-row-label">
          Enter key
          <Explainer title="Enter key">
            <p>Sends: {enter} sends your message, and {shiftEnter} adds a new line.</p>
            <p>New line: {enter} adds a new line, and {shiftEnter} sends.</p>
            <p>{modEnter} sends in both. On a phone, {enter} always adds a new line and the Send button sends.</p>
          </Explainer>
        </span>
        <div class="segmented-control" role="group" aria-label="Enter key">
          {PREF_COMPOSER_ENTER_KEY.values.map((m) => (
            <button
              key={m}
              type="button"
              aria-pressed={mode === m}
              class={`segmented-btn ${mode === m ? 'active' : ''}`}
              onClick={() => void setComposerEnterKey(m)}
            >
              {COMPOSER_ENTER_KEY_LABELS[m]}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Settings → Keyboard Shortcuts. Lists every registry shortcut with its
 *  current (possibly-customized) binding and a Record button to rebind it.
 *  Bindings are synced across devices via the `keybindings` workspace
 *  preference. */
export function KeyboardShortcutsSection() {
  const [recordingId, setRecordingId] = useState<ShortcutId | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!recordingId) return;
    // Window capture runs before every document listener, the global shortcut
    // dispatcher's Escape capture included, so no recorded chord also acts.
    function onKey(e: KeyboardEvent) {
      const outcome = recordChord(e, recordingId!);
      if (outcome.kind === 'modifier') return; // bare Ctrl/Cmd/etc — keep waiting
      e.preventDefault();
      e.stopPropagation();
      switch (outcome.kind) {
        case 'cancel':
          setError(null);
          break;
        case 'invalid':
          setError('Use Ctrl/Cmd or Alt together with a key.');
          break;
        case 'composer':
          setError(`${enterChord(false, false)}, ${enterChord(false, true)} and ${enterChord(true, false)} send or add a new line in the composer (see Enter key above).`);
          break;
        case 'conflict':
          setError(`That combination is already used by "${shortcutDef(outcome.withId).label}".`);
          break;
        case 'ok':
          setError(null);
          void setBinding(recordingId!, outcome.binding);
          break;
      }
      setRecordingId(null);
    }
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [recordingId]);

  return (
    <div class="settings-section">
      <div class="settings-section-title">Keyboard Shortcuts</div>
      <p class="settings-section-desc">
        Click a shortcut and press a new key combination. Changes are saved and synced across your devices.
      </p>
      {error && <p class="error-text">{error}</p>}
      <ComposerEnterKeyRow />
      {CATEGORIES.map((cat) => (
        <div class="settings-section" key={cat}>
          <div class="settings-section-title">{cat}</div>
          <div class="list-rows">
            {SHORTCUT_DEFS.filter((d) => d.category === cat).map((def) => (
              <div class="list-row" key={def.id} data-search-anchor={shortcutSearchAnchor(def.id)}>
                <div class="list-row-info">
                  <span class="title">{def.label}</span>
                  {/* It explains the default chord, so a rebound one hides it. */}
                  {def.mnemonic && !isCustomized(def.id) && (
                    <span class="list-row-details" data-role="shortcut-mnemonic">{def.mnemonic}</span>
                  )}
                </div>
                <div class="list-row-actions shortcut-row-actions">
                  <button
                    class="shortcut-keys shortcut-record-btn"
                    onClick={() => {
                      if (recordingId === def.id) {
                        setRecordingId(null);
                      } else {
                        setError(null);
                        setRecordingId(def.id);
                      }
                    }}
                    aria-label={
                      recordingId === def.id
                        ? `Stop recording ${def.label}`
                        : `Record a new shortcut for ${def.label}`
                    }
                  >
                    {recordingId === def.id ? 'Press keys… (Esc to cancel)' : displayBinding(def.id)}
                  </button>
                  <button
                    class="action-btn action-btn-danger"
                    disabled={!isCustomized(def.id)}
                    onClick={() => void resetBinding(def.id)}
                    aria-label={`Reset ${def.label} to its default shortcut`}
                  >
                    Reset
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
