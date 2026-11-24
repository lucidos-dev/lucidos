import { useEffect } from 'preact/hooks';
import { readAgentPause } from '../api/client';
import { agentPauseVersion } from '../store/actions/users';
import { showToast } from '../store/store';
import { useThreadLoadable } from './useThreadLoadable';

export function useAgentPaused(threadId: string | null): boolean {
  const [pause] = useThreadLoadable(threadId, agentPauseVersion.value, readAgentPause);
  const error = pause.status === 'failed' ? pause.error : null;
  useEffect(() => {
    if (error) showToast(`Could not read whether the agent is paused: ${error}`, 'error');
  }, [error]);
  return pause.status === 'loaded' && pause.data;
}
