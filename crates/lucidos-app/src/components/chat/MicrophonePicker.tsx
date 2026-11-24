import { useEffect } from 'preact/hooks';
import {
  microphones,
  nameMicrophones,
  refreshMicrophones,
  watchMicrophones,
} from '../../store/microphones';
import { preferences } from '../../store/store';
import { setVoiceInputDevice, storedVoiceInputDevice } from '../../store/actions/preferences';
import { anyMicrophoneNamed, microphoneChoices } from '../../voice/microphone';
import { isOnCall } from '../../voice/callState';
import { voiceCall } from '../../store/voiceCall';

export function MicrophoneList({ run }: { run: (fn: () => void) => (e: MouseEvent) => void }) {
  useEffect(() => {
    void refreshMicrophones();
    return watchMicrophones();
  }, []);
  preferences.value;
  const stored = storedVoiceInputDevice();
  const found = microphones.value;
  const devices = found.status === 'loaded' ? found.data : [];
  const rows = microphoneChoices(devices, stored);
  return (
    <>
      <div class="control-section-label">Microphone</div>
      {rows.map((row) => (
        <button
          key={row.value}
          type="button"
          role="menuitemradio"
          aria-checked={row.value === stored}
          data-value={row.value}
          class="thread-overflow-item"
          onClick={run(() => {
            void setVoiceInputDevice(row.value);
          })}
        >
          <span class="thread-overflow-check" aria-hidden="true">
            {row.value === stored ? '✓' : ''}
          </span>
          {row.label}
        </button>
      ))}
      {found.status === 'failed' && (
        <div class="thread-overflow-note thread-overflow-note-error" role="alert">
          {found.error}
        </div>
      )}
      {found.status === 'loaded' && !anyMicrophoneNamed(devices) && (
        <>
          <div class="thread-overflow-divider" role="separator" />
          <button
            type="button"
            role="menuitem"
            class="thread-overflow-item"
            onClick={() => {
              void nameMicrophones();
            }}
          >
            <span class="thread-overflow-check" aria-hidden="true" />
            Allow Lucidos to see your microphone names
          </button>
        </>
      )}
      {isOnCall(voiceCall.value.phase) && (
        <div class="thread-overflow-note">The call you are on keeps the microphone it opened.</div>
      )}
    </>
  );
}
