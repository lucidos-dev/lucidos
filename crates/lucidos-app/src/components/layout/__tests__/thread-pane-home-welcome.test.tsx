// @vitest-environment jsdom
/**
 * An empty Home showing the welcome takes the compose layout, with its title
 * at the pane's top (ADR 0411). A Home with a message is an ordinary thread
 * view again. The pane's children are stubbed: this pins which ones it draws.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../chat/PromptInput', () => ({ PromptInput: () => <div data-stub="prompt" /> }));
vi.mock('../../chat/ThreadView', () => ({ ThreadView: () => <div data-stub="thread-view" /> }));
vi.mock('../../chat/CreateThreadView', () => ({ CreateThreadView: () => <div data-stub="compose-view" /> }));
vi.mock('../../chat/HomeWelcomeTitle', () => ({ HomeWelcomeTitle: () => <div data-stub="home-title" /> }));

import { ThreadPane } from '../ThreadPane';
import { focusedThreadId, preferences, threadMap } from '../../../store/store';
import type { StoredEvent } from '../../../store/thread-events';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

let host: HTMLDivElement;

function seedHome(events = new Map<number, StoredEvent>()): void {
  threadMap.value = new Map([
    ['home', makeThreadState('home', { meta: { title: 'Home', home: true, section: 'inbox' }, events, eventsLoaded: true })],
  ]);
  focusedThreadId.value = 'home';
}

const drawn = (stub: string) => host.querySelector(`[data-stub="${stub}"]`) !== null;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  preferences.value = { status: 'loaded', data: {} };
});

afterEach(() => {
  render(null, host);
  host.remove();
  focusedThreadId.value = null;
  threadMap.value = new Map();
  preferences.value = { status: 'not-loaded' };
});

describe('ThreadPane on Home', () => {
  it('draws an empty Home in the compose layout, its title on top', () => {
    seedHome();
    render(<ThreadPane />, host);
    expect(host.querySelector('.thread-pane')?.classList.contains('compose-empty')).toBe(true);
    expect(drawn('home-title')).toBe(true);
    expect(drawn('compose-view')).toBe(true);
    expect(drawn('thread-view')).toBe(false);
  });

  it('draws a Home with a message as an ordinary thread view', () => {
    seedHome(new Map([[1, { type: 'MessageReceived', text: 'hi', _eventId: 'm1', created: '2026-10-10T08:00:00Z' } as unknown as StoredEvent]]));
    render(<ThreadPane />, host);
    expect(host.querySelector('.thread-pane')?.classList.contains('compose-empty')).toBe(false);
    expect(drawn('home-title')).toBe(false);
    expect(drawn('thread-view')).toBe(true);
  });
});
