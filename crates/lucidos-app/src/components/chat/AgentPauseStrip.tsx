import { useState } from 'preact/hooks';
import { readAgentPause, setAgentPause } from '../../api/client';
import { useThreadLoadable } from '../../hooks/useThreadLoadable';
import { agentPauseVersion, mentionsEnabled } from '../../store/actions/people';
import { focusedThreadId, showToast, threadMap } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';

/** The thread the strip speaks for: the focused one, once started, on a team
 *  gateway. A draft has no pause yet. */
function pausableThread(): string | null {
  const id = focusedThreadId.value;
  if (!id || !mentionsEnabled.value) return null;
  const state = threadMap.value.get(id)?.meta.state;
  return state === 'active' ? id : null;
}

/** Above the composer while the *agent pause* holds (ADR 0649): a message
 *  then stays between people. Resume, or a message mentioning @agent, ends
 *  it. Nothing draws while the agent listens. */
export function AgentPauseStrip() {
  const threadId = pausableThread();
  const [pause, land] = useThreadLoadable(threadId, agentPauseVersion.value, readAgentPause);
  const [resuming, setResuming] = useState(false);

  const resume = async () => {
    if (!threadId) return;
    setResuming(true);
    try {
      land(await setAgentPause(threadId, false));
    } catch (error) {
      showToast(`Could not resume the agent: ${errorDetail(error)}`, 'error');
    } finally {
      setResuming(false);
    }
  };

  return (
    <>
      <Disclosure open={pause.status === 'loaded' && pause.data}>
        <div class="agent-pause-strip" data-role="agent-pause-strip" role="status">
          <span class="agent-pause-strip-text">
            Agent paused. Mention <strong>@agent</strong> or
          </span>
          <button type="button" class="action-btn action-btn-secondary agent-pause-strip-resume" disabled={resuming} onClick={() => void resume()}>
            Resume
          </button>
        </div>
      </Disclosure>
      <Disclosure open={pause.status === 'failed'}>
        <div class="agent-pause-strip agent-pause-strip-failed" role="alert">
          {pause.status === 'failed' && `Could not read whether the agent is paused: ${pause.error}`}
        </div>
      </Disclosure>
    </>
  );
}
