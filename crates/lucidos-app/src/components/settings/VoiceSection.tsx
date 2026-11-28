import { ModelSelectionRow } from './ModelSelectionRow';
import { LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import type { ModelChoice } from '../../store/modelSelection';
import { preferences } from '../../store/store';
import { Explainer } from '../shared/Explainer';
import { Dropdown } from '../shared/Dropdown';
import {
  DEFAULT_VOICE_TALKER_VOICE,
  VOICE_RESIDENT_SECTIONS,
  currentVoiceTalkerModel,
  currentVoiceTalkerVoice,
  currentVoiceTranscriberModel,
  setVoiceEnabled,
  setVoiceSectionEnabled,
  setVoiceTalkerModel,
  setVoiceTalkerVoice,
  setVoiceTranscriberModel,
  voiceEnabled,
  voiceSectionEnabled,
} from '../../store/actions/preferences';

export const TALKER_MODELS = [
  { value: 'gpt-realtime-2.1', label: 'GPT Realtime 2.1' },
  { value: 'gpt-realtime-2.1-mini', label: 'GPT Realtime 2.1 mini' },
  { value: 'gpt-realtime-2', label: 'GPT Realtime 2' },
  { value: 'gpt-realtime-1.5', label: 'GPT Realtime 1.5' },
  { value: 'gpt-live-1', label: 'GPT Live 1' },
];

export const TRANSCRIBER_MODELS = [
  { value: 'gpt-live-transcribe', label: 'GPT live transcribe' },
  { value: 'gpt-realtime-whisper', label: 'GPT Realtime Whisper' },
  { value: 'gpt-transcribe', label: 'GPT transcribe' },
];

const TALKER_VOICES = [
  { value: 'marin', label: 'Marin' },
  { value: 'cedar', label: 'Cedar' },
  { value: 'alloy', label: 'Alloy' },
  { value: 'ash', label: 'Ash' },
  { value: 'ballad', label: 'Ballad' },
  { value: 'coral', label: 'Coral' },
  { value: 'echo', label: 'Echo' },
  { value: 'sage', label: 'Sage' },
  { value: 'shimmer', label: 'Shimmer' },
  { value: 'verse', label: 'Verse' },
];

function voiceModelChoices(
  models: readonly { value: string; label: string }[],
  current: string,
): ModelChoice[] {
  const rows: ModelChoice[] = models.map((m) => ({ ...m, reasoningEfforts: [] }));
  if (current && !rows.some((row) => row.value === current)) {
    rows.push({ value: current, label: current, reasoningEfforts: [] });
  }
  return rows;
}

export function TalkerModelExplainer() {
  return (
    <Explainer title="Talker model">
      <p>Which model holds the spoken conversation. Two families, and they behave differently.</p>
      <p>
        The <strong>Realtime</strong> models bill by the token. They can settle anything out
        loud: say which option you want, allow or refuse a permission, or say goodbye to
        ring off.
      </p>
      <p>
        <strong>GPT Live 1</strong> listens while it speaks, so it handles being interrupted
        better. It holds no tools, so it settles less out loud: a question it asks you can
        answer by just saying your answer, but a permission is a tap and so is ringing off.
        It bills by the minute, and that spend does not reach the usage rollup.
      </p>
    </Explainer>
  );
}

export function TranscriberModelExplainer() {
  return (
    <Explainer title="Transcriber model">
      <p>
        What turns your speech into text for the talker. The second and last model in a
        call: nothing translates and nothing summarises.
      </p>
      <p>
        The two at the top stream the transcript as you speak. The rest transcribe a turn
        once you have finished it.
      </p>
      <p>
        <strong>This row does nothing on a GPT Live call.</strong> That talker transcribes
        you itself and reads no model id here. Spoken voice still applies to both.
      </p>
    </Explainer>
  );
}

export function VoiceSection() {
  preferences.value;
  const on = voiceEnabled();
  const talker = currentVoiceTalkerModel();
  const transcriber = currentVoiceTranscriberModel();
  const talkerVoice = currentVoiceTalkerVoice();
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:voice">Voice</div>
      <div class="settings-row" data-search-anchor="models:voice-enabled">
        <span class="settings-row-label">
          Voice (experimental)
          <Explainer title="Voice (experimental)">
            <p>
              Adds a call control to your home thread's composer. Press it and you talk
              to Lucidos out loud. Speech and typing interleave in one transcript.
            </p>
            <p>
              Two models share the thread. A rented speech-to-speech <strong>talker</strong>{' '}
              holds the conversation and can change nothing, and the ordinary Lucidos
              Agent does the work behind it. So a spoken question costs a full agent
              turn, and a short call can cost several.
            </p>
            <p>
              It needs the OpenAI provider configured.
              Experimental: the shape of a call is still settling, and what it costs is
              not capped.
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
          <ModelSelectionRow
            label="Talker model"
            anchor="models:voice-talker"
            explainer={<TalkerModelExplainer />}
            models={voiceModelChoices(TALKER_MODELS, talker)}
            vocabulary={LUCIDOS_TIER_VOCABULARY}
            model={talker}
            effort={null}
            onChange={(p) => void setVoiceTalkerModel(p.model)}
          />
          <ModelSelectionRow
            label="Transcriber model"
            anchor="models:voice-transcriber"
            explainer={<TranscriberModelExplainer />}
            models={voiceModelChoices(TRANSCRIBER_MODELS, transcriber)}
            vocabulary={LUCIDOS_TIER_VOCABULARY}
            model={transcriber}
            effort={null}
            onChange={(p) => void setVoiceTranscriberModel(p.model)}
          />
          <div class="settings-row" data-search-anchor="models:voice-talker-voice">
            <span class="settings-row-label">
              Spoken voice
              <Explainer title="Spoken voice">
                <p>
                  Who the talker sounds like. It is not the language: Locale decides what
                  a call is spoken in, and this decides the voice speaking it.
                </p>
                <p>
                  The names are the talker model's own, and it refuses a call for one it
                  does not know. A change takes effect on your next call.
                </p>
              </Explainer>
            </span>
            <Dropdown
              options={TALKER_VOICES}
              value={talkerVoice}
              freeText
              placeholder={DEFAULT_VOICE_TALKER_VOICE}
              onChange={(v) => void setVoiceTalkerVoice(v)}
            />
          </div>
          <div class="settings-row" data-search-anchor="models:voice-resident-sections">
            <span class="settings-row-label">
              Resident context
              <Explainer title="Resident context">
                <p>
                  What a call already knows the moment it opens. The talker looks nothing
                  up, so this block is the whole of what it answers without waiting for
                  the agent.
                </p>
                <p>
                  Everything here is read at the start of the call and never refreshed
                  during it. So more of it means a longer wait for the first word, and an
                  answer that is older by the end of a long call.
                </p>
                <p>
                  Turn them all off and the talker opens knowing nothing. It can still ask
                  the agent for anything, which is what it does for everything else.
                </p>
              </Explainer>
            </span>
          </div>
          {VOICE_RESIDENT_SECTIONS.map((section) => (
            <div
              key={section.id}
              class="settings-row settings-row-child"
              data-search-anchor={`models:voice-section-${section.id}`}
            >
              <span class="settings-row-label">{section.title}</span>
              <label class="toggle-switch">
                <input
                  type="checkbox"
                  aria-label={section.title}
                  checked={voiceSectionEnabled(section.id)}
                  onChange={(e) =>
                    void setVoiceSectionEnabled(
                      section.id,
                      (e.currentTarget as HTMLInputElement).checked,
                    )
                  }
                />
                <span class="toggle-slider" />
              </label>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
