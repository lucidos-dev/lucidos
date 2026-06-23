import { useState, useEffect } from 'preact/hooks';
import { SHORTCUT_DEFS, shortcutDef, type ShortcutId } from '../../utils/shortcuts';
import { displayBinding, isCustomized, setBinding, resetBinding, recordChord } from '../../store/actions/keybindings';

// Derived, not hand-maintained: a category added to the registry can't be
// silently missing from the sheet. Order = first appearance in SHORTCUT_DEFS.
const CATEGORIES = [...new Set(SHORTCUT_DEFS.map((d) => d.category))];

/** Settings → Keyboard Shortcuts. Lists every registry shortcut with its
 *  current (possibly-customized) binding and a Record button to rebind it.
 *  Bindings are synced across devices via the `keybindings` workspace
 *  preference. */
export function KeyboardShortcutsSection() {
  const [recordingId, setRecordingId] = useState<ShortcutId | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!recordingId) return;
    // Capture phase so the recorded chord is intercepted before the global
    // shortcut dispatcher (which runs in the bubble phase) can fire its action.
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
    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [recordingId]);

  return (
    <div class="settings-section">
      <div class="settings-section-title">Keyboard Shortcuts</div>
      <p class="settings-section-desc">
        Click a shortcut and press a new key combination. Changes are saved and synced across your devices.
      </p>
      {error && <p class="error-text">{error}</p>}
      {CATEGORIES.map((cat) => (
        <div class="settings-section" key={cat}>
          <div class="settings-section-title">{cat}</div>
          <div class="list-rows">
            {SHORTCUT_DEFS.filter((d) => d.category === cat).map((def) => (
              <div class="list-row" key={def.id}>
                <div class="list-row-info">
                  <span class="title">{def.label}</span>
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
