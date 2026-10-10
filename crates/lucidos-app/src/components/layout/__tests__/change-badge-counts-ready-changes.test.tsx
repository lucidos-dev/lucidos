/**
 * The Changes badge counts only the changes the reader can act on now.
 *
 * A change whose thread has not settled offers Apply on settle and nothing
 * else, so it owes the reader nothing yet. The menu hamburger leads to the
 * Changes row, so it carries the same count.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { HamburgerButton } from '../ContentNav';
import { SystemAttentionBadge } from '../../shared/SystemAttentionBadge';
import { findByClass, textOf, type AnyVNode } from './vnodeWalk';
import { actionableChangeCount, changes, releaseNoticeView } from '../../../store/store';
import { drawerClosing, drawerOpen } from '../drawerState';
import type { Change } from '../../../api/client';

function change(id: string, threadUnsettled: boolean): Change {
  return { id, thread_unsettled: threadUnsettled } as Change;
}

function hamburger(): AnyVNode {
  return HamburgerButton() as AnyVNode;
}

beforeEach(() => {
  changes.value = { status: 'not-loaded' };
  releaseNoticeView.value = { status: 'not-loaded' };
  drawerOpen.value = false;
  drawerClosing.value = false;
});

describe('the actionable change count', () => {
  it('leaves out a change still waiting on its thread', () => {
    changes.value = {
      status: 'loaded',
      data: [change('a', false), change('b', true), change('c', true)],
    };
    expect(actionableChangeCount.value).toBe(1);
  });

  it('is unknown until the list loads, never a zero', () => {
    expect(actionableChangeCount.value).toBe(null);
    changes.value = { status: 'failed', error: 'down' };
    expect(actionableChangeCount.value).toBe(null);
  });
});

describe('the menu hamburger', () => {
  it('draws no count while every change waits on its thread', () => {
    changes.value = { status: 'loaded', data: [change('a', true)] };
    expect(findByClass(hamburger(), 'badge')).toHaveLength(0);
    expect(hamburger().props['aria-label']).toBe('Open menu');
  });

  it('draws the count, and says it in its label', () => {
    changes.value = {
      status: 'loaded',
      data: [change('a', false), change('b', false), change('c', true)],
    };
    const [badge] = findByClass(hamburger(), 'badge');
    expect(textOf(badge)).toBe('2');
    expect(hamburger().props['aria-label']).toBe('Open menu · 2 changes ready');
  });

  it('gives the corner to the count and keeps the System news in the label', () => {
    changes.value = { status: 'loaded', data: [change('a', false)] };
    releaseNoticeView.value = {
      status: 'loaded',
      data: {
        notices: [{ id: 'n', since: '2.0.0', title: 'Audit', body: 'Run it.', resolved: false, action_deferred: false }],
        next_id: 'n',
      },
    };
    const kids = hamburger().props.children as AnyVNode[];
    const dot = kids.find((k) => k?.type === SystemAttentionBadge);
    expect(dot?.props.label).toBe(null);
    expect(hamburger().props['aria-label']).toBe('Open menu · 1 change ready · 1 thing to do');
  });
});
