// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it } from 'vitest';
import { Fragment, render } from 'preact';
import { PromptActionsRight } from '../PromptRowControls';

const CodingAgentPicker = () => <div class="rung-pill" data-picker="coding-agent" />;
const ChatPicker = () => <div class="rung-pill" data-picker="chat" />;

function Row({ diffApplyStand, chat }: { diffApplyStand: boolean; chat: boolean }) {
  const cluster = [
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
