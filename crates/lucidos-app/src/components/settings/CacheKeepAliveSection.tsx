import {
  CACHE_KEEP_ALIVE_MODES,
  currentCacheKeepAliveMode,
  setCacheKeepAliveMode,
  type CacheKeepAliveMode,
} from '../../store/actions/preferences';
import { openCacheKeepAliveSetting } from '../../store/actions/menu';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

const CACHE_KEEP_ALIVE_COPY: Record<CacheKeepAliveMode, { label: string; description: string }> = {
  'break-even': {
    label: 'Break-even',
    description: 'Gives up once the pings cost one cache rewrite',
  },
  'workspace-data': {
    label: 'Tuned to this workspace',
    description: 'Gives up where your last 30 days of waits cost least',
  },
  off: {
    label: 'Off',
    description: 'No pings: every long wait pays a full cache rewrite',
  },
};

const DEFAULT_CACHE_KEEP_ALIVE = PREFERENCE_CATALOG.cache_keep_alive_mode.fallback;

/** The picker's options, the default first, each with its trade-off line. */
export const CACHE_KEEP_ALIVE_OPTIONS: DropdownOption[] = [
  DEFAULT_CACHE_KEEP_ALIVE,
  ...CACHE_KEEP_ALIVE_MODES.filter((mode) => mode !== DEFAULT_CACHE_KEEP_ALIVE),
].map((mode) => ({ value: mode, ...CACHE_KEEP_ALIVE_COPY[mode] }));

/** Guards against a value the engine would reject, so a stale or hand-edited
 *  preference cannot be re-saved from here. */
export function isCacheKeepAliveMode(value: string): value is CacheKeepAliveMode {
  return (CACHE_KEEP_ALIVE_MODES as readonly string[]).includes(value);
}

/**
 * Settings → Models → Cache keep-alive: when the engine stops pinging a waiting
 * Claude Code session or Lucidos Agent thread to keep its prompt cache warm.
 *
 * Writes through `setCacheKeepAliveMode`, which applies locally before the
 * request and parks a failed delivery for retry.
 */
export function CacheKeepAliveSection() {
  const mode = currentCacheKeepAliveMode();

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:cache-keep-alive">
        Cache keep-alive
        <Explainer title="Cache keep-alive">
          <p>
            A model's prompt cache expires minutes after its last request. A thread that
            waits longer re-writes its whole context when it wakes, which costs many times
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
            <strong>{CACHE_KEEP_ALIVE_COPY['break-even'].label}</strong> stops
            once the pings would cost one rewrite, and at half of that for a card: an
            answer that late is rare.
          </p>
          <p>
            <strong>{CACHE_KEEP_ALIVE_COPY['workspace-data'].label}</strong> picks the stop
            point per kind of wait from this workspace's last 30 days. A kind with fewer
            than 50 waits keeps break-even.
          </p>
          <p>
            <strong>{CACHE_KEEP_ALIVE_COPY.off.label}</strong>, the default, sends no pings.
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
            <Dropdown
              options={CACHE_KEEP_ALIVE_OPTIONS}
              value={mode}
              onChange={(v) => {
                if (isCacheKeepAliveMode(v)) void setCacheKeepAliveMode(v);
              }}
            />
          </div>
        </div>
      </div>
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
        <span class="settings-row-label">How long Lucidos keeps a waiting session's prompt cache warm</span>
        <button class="action-btn" onClick={openCacheKeepAliveSetting}>
          Open Models
        </button>
      </div>
    </div>
  );
}
