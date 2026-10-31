/** The cache cards are a sub-setting of the keep-alive: they draw only while
 *  the keep-alive is on, whatever their own switch says. */
import { describe, it, expect, afterEach } from 'vitest';
import { currentCacheCards, currentCacheKeepAliveOn } from '../../store/actions/preferences';
import { preferences } from '../../store/store';

function setPrefs(data: Record<string, string>) {
  preferences.value = { status: 'loaded', data };
}

afterEach(() => {
  preferences.value = { status: 'not-loaded' };
});

describe('cache keep-alive switch', () => {
  it('is off by default', () => {
    setPrefs({});
    expect(currentCacheKeepAliveOn()).toBe(false);
  });

  it('reads on only for the on value', () => {
    setPrefs({ cache_keep_alive_mode: 'on' });
    expect(currentCacheKeepAliveOn()).toBe(true);
    setPrefs({ cache_keep_alive_mode: 'off' });
    expect(currentCacheKeepAliveOn()).toBe(false);
  });
});

describe('cache cards', () => {
  it('draw while the keep-alive and the cards are both on', () => {
    setPrefs({ cache_keep_alive_mode: 'on', cache_cards: 'true' });
    expect(currentCacheCards()).toBe(true);
  });

  it('never draw while the keep-alive is off', () => {
    setPrefs({ cache_keep_alive_mode: 'off', cache_cards: 'true' });
    expect(currentCacheCards()).toBe(false);
    setPrefs({ cache_cards: 'true' });
    expect(currentCacheCards()).toBe(false);
  });

  it('stay hidden while their own switch is off', () => {
    setPrefs({ cache_keep_alive_mode: 'on', cache_cards: 'false' });
    expect(currentCacheCards()).toBe(false);
  });
});
