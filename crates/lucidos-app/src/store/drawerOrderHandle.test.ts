import { describe, it, expect } from 'vitest';
import { activeViewKey, orderedVisibleThreadIds, provideDrawerOrder } from './drawerOrderHandle';
import * as drawerOrder from '../components/drawer/drawerOrder';
import { visibleCandidatesAround } from './actions/threads';
import { threadMap } from './store';
import { makeThreadState } from './actions/threads-test-helpers';

describe('drawer order handle', () => {
  it('answers an empty order before the shell provides the drawer', () => {
    threadMap.value = new Map([['a', makeThreadState('a')], ['b', makeThreadState('b')]]);
    expect(orderedVisibleThreadIds()).toEqual([]);
    expect(activeViewKey()).not.toBe(drawerOrder.activeViewKey());
    expect(visibleCandidatesAround('a')).toEqual([]);
  });

  it('forwards to the drawer once provided', () => {
    provideDrawerOrder(drawerOrder);
    expect(orderedVisibleThreadIds()).toEqual(drawerOrder.orderedVisibleThreadIds());
    expect(activeViewKey()).toBe(drawerOrder.activeViewKey());
  });
});
