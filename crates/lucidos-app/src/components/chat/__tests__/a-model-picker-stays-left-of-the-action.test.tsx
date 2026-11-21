// @vitest-environment jsdom
/** The model picker stands left of the action slot, whatever the row does.
 *
 *  Merged, Diff and the standing apply leave a `null` member in the cluster.
 *  A switch from a coding-agent thread to a Lucidos Agent thread can drop them
 *  and swap the picker's component in one render. Preact then appended the
 *  picker after the Send button. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { Fragment, render } from 'preact';
import { PromptActionsRight } from '../PromptRowControls';

/** Two picker components, as a coding agent's and the Lucidos Agent's are. */
const CodingAgentPicker = () => <div class="rung-pill" data-picker="coding-agent" />;
const ChatPicker = () => <div class="rung-pill" data-picker="chat" />;

function Row({ diffApplyStand, chat }: { diffApplyStand: boolean; chat: boolean }) {
  const cluster = [
    // The merged pair: the first draws the merged control, the second nothing.
    ...(diffApplyStand ? [<Fragment key="diff"><button class="diff" /></Fragment>, null] : []),
    <Fragment key="rung">{chat ? <ChatPicker /> : <CodingAgentPicker />}</Fragment>,
  ];
  return <PromptActionsRight cluster={cluster} action={<button key="send-cancel-morph" class="action-slot" />} />;
}

let host: HTMLDivElement;
beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});
afterEach(() => {
  render(null, host);
  host.remove();
});

/** The picker and the action slot, in document order. */
const order = () => [...host.querySelectorAll('.rung-pill, .action-slot')].map((el) => el.className);

it('keeps the picker left of Send when Diff and apply leave as the picker swaps', () => {
  render(<Row diffApplyStand chat={false} />, host);
  render(<Row diffApplyStand={false} chat />, host);
  expect(order()).toEqual(['rung-pill', 'action-slot']);
});

it('keeps the action slot the right-hand group\'s last child', () => {
  render(<Row diffApplyStand chat={false} />, host);
  render(<Row diffApplyStand={false} chat />, host);
  expect(host.querySelector('.prompt-actions-right > :last-child')?.className).toBe('action-slot');
});
