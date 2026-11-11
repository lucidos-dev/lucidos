import { operators, personName } from '../../store/actions/operators';
import { getDeviceId } from '../../store/actions/devices';
import type { AgentPauseCause, MessageOrigin } from '../../generated/thread-event-wire';
import { eventNameChip, eventRowBody } from './EventRow';

interface Props {
  paused: boolean;
  cause: AgentPauseCause;
  actor?: MessageOrigin;
}

/** Who flipped the pause, as the row names them. */
function flippedBy(actor: MessageOrigin | undefined): string | null {
  if (actor?.kind !== 'device') return null;
  if (actor.device_id === getDeviceId()) return 'you';
  return personName(operators.value, actor.device_id);
}

/** What the row says about why the pause flipped. */
export function agentPauseState(paused: boolean, cause: AgentPauseCause, who: string | null): string {
  const by = who ? `by ${who}` : null;
  if (cause === 'chip') return by ?? '';
  const how = paused ? 'mentioning only people' : 'mentioning @agent';
  return by ? `${by}, ${how}` : how;
}

/** An `AgentPaused` or `AgentResumed` in the transcript (ADR 0649). The
 *  messages between a pause and the next resume stay between people. */
export function AgentPauseRow({ paused, cause, actor }: Props) {
  return eventRowBody({
    kind: 'boundary',
    role: 'agent-pause',
    subject: eventNameChip({
      kind: 'chip',
      name: paused ? 'AgentPaused' : 'AgentResumed',
      label: paused ? 'agent paused' : 'agent resumed',
      meaning: paused
        ? 'Messages stay between people until someone mentions @agent. The agent reads them on its next turn.'
        : 'Messages wake the agent again.',
      sentenceStart: true,
    }),
    stateLabel: agentPauseState(paused, cause, flippedBy(actor)),
  });
}
