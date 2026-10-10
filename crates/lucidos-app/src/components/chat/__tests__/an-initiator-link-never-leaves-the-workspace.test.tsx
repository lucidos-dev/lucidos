// @vitest-environment jsdom
// The turn is rendered for real: markdown needs a DOM to sanitize against, and
// a delegated click is only a delegated click once the elements exist.
/** A link in the turn's INITIATOR body is routed, exactly like one in a reply.
 *
 *  `handleLinkClick` was wired to the two `.response-content` divs and nowhere
 *  else, so every anchor the initiator panel drew went to the browser. There
 *  are no relative routes: the SPA fallback answers with the shell and the
 *  whole workspace reloads. ADR 0038's terminal guard exists to stop that, and
 *  this body never reached it.
 *
 *  Both reported shapes are pinned. A relative markdown anchor is the reload,
 *  and a linkified artifact path is the dead link: it wears the affordance and
 *  nothing reads its `data-path`.
 *
 *  `preventDefault` is the assertion because it is the reload, not a proxy for
 *  it. The branch each click lands in is `chat-link-click.test.ts`'s subject.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { ChatExchange } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

const THREAD = 'tid';

/** The reader's own typed message, the plainest body this panel draws. */
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
      threadCodingAgent="claude-code"
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

/** A click as the browser delivers one, cancelable so the guard has something
 *  to cancel. */
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
