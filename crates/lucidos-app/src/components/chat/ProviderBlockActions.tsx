import { useState } from 'preact/hooks';
import { getCodingAgentBinaries } from '../../api/client';
import type { SuccessorImplementer } from '../../api/client';
import type { CodingAgent } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { showToast } from '../../store/store';
import { continueInSuccessor } from '../../store/actions/successor';
import { errorDetail } from '../../utils/errorDetail';

/** The two ways on from a turn OpenAI blocked. Continue starts a new session
 *  in this thread from a brief. Switch does the same on Claude Code, and only
 *  shows once the engine reports a usable `claude` binary: a probe that failed
 *  or found none hides it, since the engine would refuse the switch. */
export function ProviderBlockActions({ threadId, sessionCodingAgent }: { threadId: string; sessionCodingAgent: CodingAgent }) {
  const [busy, setBusy] = useState(false);
  const { loadable: binaries } = useLoadableFetch(getCodingAgentBinaries, []);
  const claudeUsable = sessionCodingAgent !== 'claude-code' && binaries.status === 'loaded' && binaries.data.claude_code.valid;

  async function proceed(implementer?: SuccessorImplementer): Promise<void> {
    setBusy(true);
    try {
      await continueInSuccessor(threadId, implementer);
    } catch (err) {
      showToast(`Could not continue in a new session: ${errorDetail(err)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="exchange-error-actions">
      {claudeUsable && (
        <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => void proceed({ coding_agent: 'claude-code' })}>
          Switch to Claude Code
        </button>
      )}
      <button type="button" class="action-btn" disabled={busy} onClick={() => void proceed()}>
        Continue in a new session
      </button>
    </div>
  );
}
