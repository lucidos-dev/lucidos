import {
  currentCacheCards,
  currentCacheKeepAliveOn,
  setCacheCards,
  setCacheKeepAliveOn,
} from '../../store/actions/preferences';
import { openCacheKeepAliveSetting } from '../../store/actions/menu';
import { preferences } from '../../store/store';
import { Disclosure } from '../shared/Disclosure';
import { Explainer } from '../shared/Explainer';
import { LoadableToggle } from '../shared/LoadableToggle';

/**
 * Settings → Models → Cache keep-alive: whether the engine pings a waiting
 * Claude Code session or Lucidos Agent thread to keep its prompt cache warm.
 * The Cache cards switch nests under it and shows only while it is on, since
 * the cards never draw while it is off.
 *
 * Both write through `savePreference`, which applies locally before the
 * request and parks a failed delivery for retry.
 */
export function CacheKeepAliveSection() {
  const loaded = preferences.value.status === 'loaded';
  const on = currentCacheKeepAliveOn();

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:cache-keep-alive">
        Cache keep-alive
        <Explainer title="Cache keep-alive">
          <p>
            A model's prompt cache expires minutes after its last request. A thread that
            waits longer rereads its whole context when it wakes, which costs many times
            a cached read. So while a thread waits, Lucidos pings the model with a tiny
            request just before the cache expires.
          </p>
          <p>
            It pings a thread waiting on an event, on a sub-thread, or on your answer to a
            question card or permission prompt. For Claude Code it asks a throwaway copy of
            the session. For the Lucidos Agent it sends the start of the last request again
            and asks for no reply. Either way the conversation stays as it is.
          </p>
          <p>
            It pings toward when the thread expects to wake. The agent states that, or
            Lucidos estimates it. It sends no pings when the wake lies too far off to be
            worth it, and never more than one reread's worth for a wait. With no estimate
            it stops at that reread cap, and at half of it for a card. Off, the default,
            sends no pings.
          </p>
        </Explainer>
      </div>
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Claude Code and the Lucidos Agent</div>
            <div class="list-row-details list-row-details-prose">
              Codex threads, and models whose cache a ping cannot keep warm, are unaffected.
            </div>
          </div>
          <div class="list-row-actions">
            <LoadableToggle
              loaded={loaded}
              checked={on}
              ariaLabel="Cache keep-alive"
              onChange={(checked) => void setCacheKeepAliveOn(checked)}
            />
          </div>
        </div>
      </div>
      <Disclosure open={on}>
        <div class="settings-row settings-row-child" data-search-anchor="models:cache-cards">
          <span class="settings-row-label">
            Cache cards
            <Explainer title="Cache cards">
              <p>
                Show the cards about a waiting thread's prompt cache: Cache kept
                warm, and the cache-warm line with how long the cache stays warm.
                Reply before the time runs out and the next turn reads the cache
                cheaply.
              </p>
              <p>Off by default. Pings and costs do not change.</p>
            </Explainer>
          </span>
          <LoadableToggle
            loaded={loaded}
            checked={currentCacheCards()}
            onChange={(checked) => void setCacheCards(checked)}
          />
        </div>
      </Disclosure>
    </div>
  );
}

/** Coding Agents' one-line pointer to the setting, which lives on Models since
 *  it covers the Lucidos Agent too (ADR 0556). */
export function CacheKeepAlivePointer() {
  return (
    <div class="settings-section">
      <div class="settings-section-title">Cache keep-alive</div>
      <div class="settings-row">
        <span class="settings-row-label">Whether Lucidos keeps a waiting session's prompt cache warm</span>
        <button class="action-btn" onClick={openCacheKeepAliveSetting}>
          Open Models
        </button>
      </div>
    </div>
  );
}
