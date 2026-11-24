import { showConfirm } from '../../store/store';
import { rungSwitchCopy, type RungSwitchCopyInput } from './rungSwitchCopy';

export type RungSwitchChoice = 'handoff' | 'keep' | 'switch';

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
      alternative: { label: copy.switchHereLabel, detail: copy.rewrite, onClick: () => { choice = 'switch'; } },
    },
  });
  return confirmed ? 'handoff' : choice;
}

interface SendGate {
  asks(threadId: string): boolean;
  proceeds(threadId: string): Promise<boolean>;
}

const sendGates = new Set<SendGate>();

export function registerSendGate(gate: SendGate): () => void {
  sendGates.add(gate);
  return () => {
    sendGates.delete(gate);
  };
}

export function sendGateAsks(threadId: string): boolean {
  return [...sendGates].some((gate) => gate.asks(threadId));
}

export async function passSendGate(threadId: string): Promise<boolean> {
  for (const gate of [...sendGates]) {
    if (gate.asks(threadId) && !await gate.proceeds(threadId)) return false;
  }
  return true;
}
