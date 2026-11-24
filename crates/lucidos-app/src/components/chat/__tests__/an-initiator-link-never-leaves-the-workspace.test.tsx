// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { ChatExchange } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

const THREAD = 'tid';

function userTurn(text: string): Exchange {
  return {
    userEvent: {
      type: 'MessageReceived',
      text,
      mode: 'human',
      channel: 'chat',
      created: '2026-01-01T12:00:00Z',
      _eventId: 'u-1',
    },
    userSeq: 1,
    steps: [],
  };
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

function drawTurn(text: string): void {
  render(
    <ChatExchange
      exchange={userTurn(text)}
      revision={0}
      streamingBuffer=""
      isLast={false}
      threadId={THREAD}
      threadIsCC={false}
      sessionCodingAgent="claude-code"
      threadIdle={true}
      threadAwaitingAnswer={false}
      threadCanceling={false}
    />,
    host,
  );
}

function bodyAnchor(): HTMLAnchorElement {
  const el = host.querySelector('.initiator-body a');
  expect(el, 'the initiator body drew no anchor').not.toBeNull();
  return el as HTMLAnchorElement;
}

function clickIt(el: Element): MouseEvent {
  const e = new MouseEvent('click', { bubbles: true, cancelable: true });
  el.dispatchEvent(e);
  return e;
}

describe('a link in the initiator body', () => {
  it('is swallowed when it is relative, rather than reloading the workspace', () => {
    drawTurn('See [the plan](docs/plan.md).');
    const anchor = bodyAnchor();
    expect(anchor.getAttribute('href')).toBe('docs/plan.md');
    expect(clickIt(anchor).defaultPrevented).toBe(true);
  });

  it('is claimed when the linkifier made it an artifact link', () => {
    drawTurn('Read artifacts/reports/q3.pdf for the numbers.');
    const anchor = bodyAnchor();
    expect(anchor.className).toContain('artifact-link');
    expect(anchor.dataset.path).toBe('artifacts/reports/q3.pdf');
    expect(clickIt(anchor).defaultPrevented).toBe(true);
  });
});
