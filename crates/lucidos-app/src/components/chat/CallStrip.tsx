/**
 * What a live call looks like, above the prompt row.
 *
 * It reports and carries no control: ending is the toggle's job, and a second
 * end button would be a second thing to find. Nothing about a call is written
 * down this phase, so this is the only place a call is visible.
 *
 * The state label is a `status` region, so a screen reader hears the call go
 * up and come down. The transcript is not, because announcing every delta of a
 * reply that is already being spoken aloud would talk over it.
 */
import { callStatusLabel, isOnCall } from '../../voice/callState';
import { voiceCall } from '../../store/voice';

export function CallStrip() {
  const call = voiceCall.value;
  if (!isOnCall(call.phase)) return null;
  return (
    <div class="call-strip" data-phase={call.phase} data-role="call-strip">
      <span class="call-strip-state" role="status">
        <span class="call-strip-dot" aria-hidden="true" />
        {callStatusLabel(call.phase)}
      </span>
      {call.heard && (
        <span class="call-strip-line call-strip-heard">
          <span class="call-strip-who">You</span>
          {call.heard}
        </span>
      )}
      {call.said && (
        <span class="call-strip-line call-strip-said">
          <span class="call-strip-who">Lucidos</span>
          {call.said}
        </span>
      )}
    </div>
  );
}
