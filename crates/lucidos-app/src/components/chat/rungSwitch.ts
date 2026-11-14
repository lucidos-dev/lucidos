import { showConfirm } from '../../store/store';
import { rungSwitchCopy, type RungSwitchCopyInput } from './rungSwitchCopy';

/** What the user chose in the confirm a cache-rebuilding switch raises. Only
 *  `switch` sends anything to the live session. */
export type RungSwitchChoice = 'handoff' | 'keep' | 'switch';

/** Ask whether to hand off, keep the rung or switch anyway. When `canHandOff`,
 *  the handoff and the switch are two confirm choices, the handoff first.
 *  Escape and a click outside keep the rung, like the Keep button. */
export async function askRungSwitch(input: RungSwitchCopyInput): Promise<RungSwitchChoice> {
  const copy = rungSwitchCopy(input);
  if (!input.canHandOff) {
    const confirmed = await showConfirm(copy.message, copy.switchLabel, { variant: 'default', cancelLabel: copy.keepLabel });
    return confirmed ? 'switch' : 'keep';
  }
  let choice: RungSwitchChoice = 'keep';
  const confirmed = await showConfirm(copy.message, copy.handoffLabel, {
    variant: 'default',
    title: copy.title,
    cancelLabel: copy.keepLabel,
    choices: {
      okDetail: copy.handoffDetail,
      // `ConfirmDialog` settles after this handler returns, so it only notes
      // the choice. A dialog opened from here would be the one it closes.
      alternative: { label: copy.switchHereLabel, detail: copy.rewrite, onClick: () => { choice = 'switch'; } },
    },
  });
  return confirmed ? 'handoff' : choice;
}

/** A check a send in a started thread may owe before it goes out. A rung
 *  control registers one while it is mounted. */
interface SendGate {
  /** Whether the send has a question to ask. Synchronous, so a send with
   *  nothing to ask is not delayed a tick: the composer empties on the press. */
  asks(threadId: string): boolean;
  /** Ask it. `false` cancels the send. */
  proceeds(threadId: string): Promise<boolean>;
}

const sendGates = new Set<SendGate>();

/** Register the check, and return the call that withdraws it. */
export function registerSendGate(gate: SendGate): () => void {
  sendGates.add(gate);
  return () => {
    sendGates.delete(gate);
  };
}

/** Whether any registered check has a question for a send in `threadId`. */
export function sendGateAsks(threadId: string): boolean {
  return [...sendGates].some((gate) => gate.asks(threadId));
}

/** Whether a send in `threadId` may go out: every check that asks agrees. */
export async function passSendGate(threadId: string): Promise<boolean> {
  for (const gate of [...sendGates]) {
    if (gate.asks(threadId) && !await gate.proceeds(threadId)) return false;
  }
  return true;
}
