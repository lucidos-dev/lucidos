/**
 * Which microphone a call opens, chosen by holding the call control.
 *
 * Held rather than tapped, because the tap is the call. The picker is the
 * second thing the control does, and a second button beside it would be a
 * second thing to find.
 *
 * **Nothing here can end a call, and nothing here changes one.** A pick reaches
 * the microphone the next call opens, so the menu says so while one is live.
 * Swapping mid-call would mean tearing the capture graph off a context the
 * playback is riding.
 */
import { useEffect } from 'preact/hooks';
import { OverflowMenu, type OverflowMenuOpener } from '../shared/OverflowMenu';
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
import { voiceCall } from '../../store/voice';

/** Mounted only while the menu is open, which is what makes the device read and
 *  the hardware listener cost nothing the rest of the time. */
function MicrophoneList({ run }: { run: (fn: () => void) => (e: MouseEvent) => void }) {
  useEffect(() => {
    void refreshMicrophones();
    return watchMicrophones();
  }, []);
  // Subscribe to the preference signal: the checkmark follows what is stored,
  // including a value another device wrote.
  preferences.value;
  const stored = storedVoiceInputDevice();
  const found = microphones.value;
  const devices = found.status === 'loaded' ? found.data : [];
  // Drawn from the first frame, with no loading row. This popover's body IS
  // its values, so a placeholder would only resize it on settle. The default
  // row is true before any device is read, and the rest arrive beneath it.
  const rows = microphoneChoices(devices, stored);
  return (
    <>
      <div class="control-section-label">Microphone</div>
      {rows.map((row) => (
        <button
          key={row.value}
          type="button"
          // A radio rather than a plain item: these rows record a choice, and
          // exactly one of them is the current one.
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
            // NOT through `run`, which closes the menu before firing. This row
            // fills the list the reader is looking at. Closing first would land
            // the names in a menu that is already gone.
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

export function MicrophonePicker({
  openRef,
}: {
  /** Host-opened mode. The call control's long press is the only way in, so
   *  this menu draws no trigger of its own. */
  openRef: { current: OverflowMenuOpener | null };
}) {
  return (
    <OverflowMenu
      ariaLabel="Choose a microphone"
      stopPropagation
      openRef={openRef}
      items={({ run }) => <MicrophoneList run={run} />}
    />
  );
}
