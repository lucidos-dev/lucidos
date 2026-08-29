/**
 * The switch voice runs behind, and the two preferences it runs on.
 *
 * Voice is experimental and ships OFF. The switch is the first row, because
 * the two below it do nothing until it is on, and because turning it on is a
 * decision: it rents a speech-to-speech model, and every spoken utterance
 * starts an ordinary agent turn.
 *
 * Text fields rather than the model picker every other row uses. A realtime
 * model is deliberately not a chat-model registry row, because it cannot serve
 * an ordinary turn, so the registry has nothing to offer here.
 */
import { useEffect } from 'preact/hooks';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { preferences } from '../../store/store';
import { Explainer } from '../shared/Explainer';
import {
  DEFAULT_VOICE_RESIDENT_SECTIONS,
  DEFAULT_VOICE_TALKER_MODEL,
  storedVoiceResidentSections,
  storedVoiceTalkerModel,
  setVoiceEnabled,
  setVoiceResidentSections,
  setVoiceTalkerModel,
  voiceEnabled,
} from '../../store/actions/preferences';

/**
 * One preference on one line, saved when the field is left or Enter is pressed.
 *
 * Untouched it holds no copy, so a value another device writes repaints it.
 * Saving on blur rather than per keystroke: a model id is unusable half typed,
 * and a write per character would be a write per character.
 */
function VoiceTextRow({
  label,
  anchor,
  placeholder,
  value,
  save,
}: {
  label: string;
  anchor: string;
  placeholder: string;
  value: string;
  save: (next: string) => Promise<void>;
}) {
  const [draft, setDraft] = useServerBackedField(value);
  // Re-arm once the save lands, exactly as `LocalProviderSettings` does. A
  // field left touched keeps its draft for good: it would ignore what another
  // device writes, and the next blur would save the stale draft over it. The
  // effect runs after the render where the stored value caught up, which is
  // the only place `setDraft` can see the two agree.
  useEffect(() => {
    if (draft.trim() === value.trim()) setDraft(value);
  }, [value, draft]);
  const commit = (): void => {
    const next = draft.trim();
    // An emptied field means "back to the default", which is what saving an
    // empty string does: every reader falls back when the value is blank.
    if (next !== value.trim()) void save(next);
  };
  return (
    <div class="settings-row" data-search-anchor={anchor}>
      <span class="settings-row-label">{label}</span>
      <input
        type="text"
        class="settings-text-input"
        aria-label={label}
        placeholder={placeholder}
        value={draft}
        onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
        }}
      />
    </div>
  );
}

export function VoiceSection() {
  // Subscribe to the preference signal.
  preferences.value;
  const on = voiceEnabled();
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:voice">Voice</div>
      <div class="settings-row" data-search-anchor="models:voice-enabled">
        <span class="settings-row-label">
          Voice (experimental)
          <Explainer title="Voice (experimental)">
            <p>
              Adds a call control to the composer. Press it and you talk to Lucidos out
              loud, on the thread you are already in. Speech and typing interleave in
              one transcript.
            </p>
            <p>
              Two models share the thread. A rented speech-to-speech <strong>talker</strong>{' '}
              holds the conversation and can change nothing, and the ordinary Lucidos
              Agent does the work behind it. So a spoken question costs a full agent
              turn, and a short call can cost several.
            </p>
            <p>
              It needs the OpenAI provider configured. Experimental: the shape of a call
              is still settling, and what it costs is not capped.
            </p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={on}
            onChange={(e) => void setVoiceEnabled((e.currentTarget as HTMLInputElement).checked)}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      {on && (
        <>
          <VoiceTextRow
            label="Talker model"
            anchor="models:voice-talker"
            placeholder={DEFAULT_VOICE_TALKER_MODEL}
            value={storedVoiceTalkerModel()}
            save={setVoiceTalkerModel}
          />
          <VoiceTextRow
            label="Resident context"
            anchor="models:voice-resident-sections"
            placeholder={DEFAULT_VOICE_RESIDENT_SECTIONS}
            value={storedVoiceResidentSections()}
            save={setVoiceResidentSections}
          />
        </>
      )}
    </div>
  );
}
