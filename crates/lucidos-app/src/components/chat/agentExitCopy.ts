import type { CodingAgentExit } from '../../store/thread-events';
import type { CodingAgent } from '../../api/types';

export const CODING_AGENT_NAME: Record<CodingAgent, string> = { 'claude-code': 'Claude Code', codex: 'Codex' };

/** What the failure card says about a coding agent that exited on its own. The
 *  CLI's own words are shown apart, in `.exchange-error-output`. */
export function agentExitCopy(exit: CodingAgentExit, agent: CodingAgent): { heading: string; reason: string } {
  const name = CODING_AGENT_NAME[agent];
  const exited = exit.code === undefined ? 'It exited' : `It exited with code ${exit.code}`;
  if (exit.stage === 'startup') {
    return {
      heading: `${name} could not start`,
      reason: `${exited} before its session began. Lucidos did not retry, because a start failure repeats until its cause is fixed.`,
    };
  }
  return {
    heading: `${name} stopped unexpectedly`,
    reason: `${exited} in the middle of a reply. Lucidos did not retry it.`,
  };
}
