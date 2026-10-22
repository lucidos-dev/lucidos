import {
  CACHE_KEEP_ALIVE_MODES,
  currentCacheKeepAliveMode,
  setCacheKeepAliveMode,
  type CacheKeepAliveMode,
} from '../../store/actions/preferences';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

const CACHE_KEEP_ALIVE_COPY: Record<CacheKeepAliveMode, { label: string; description: string }> = {
  default: {
    label: 'Default',
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
 * Settings → Coding Agents → Cache keep-alive: when the engine stops pinging a
 * waiting Claude Code session to keep its prompt cache warm.
 *
 * Writes through `setCacheKeepAliveMode`, which applies locally before the
 * request and parks a failed delivery for retry.
 */
export function CacheKeepAliveSection() {
  const mode = currentCacheKeepAliveMode();

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:cache-keep-alive">
        Cache keep-alive
        <Explainer title="Cache keep-alive">
          <p>
            Claude Code's prompt cache expires minutes after its last request. A session
            that waits longer re-writes its whole context when it wakes, which costs many
            times a cached read. So while a session waits, Lucidos pings it with a tiny
            request just before the cache expires.
          </p>
          <p>
            It pings a session waiting on an event, on a sub-thread, or on your answer to
            a question card or permission prompt. For a card it asks a throwaway copy of
            the session, so the card stays as it is.
          </p>
          <p>
            <strong>{CACHE_KEEP_ALIVE_COPY[DEFAULT_CACHE_KEEP_ALIVE].label}</strong> stops
            once the pings would cost one rewrite, and at half of that for a card: an
            answer that late is rare.
          </p>
          <p>
            <strong>{CACHE_KEEP_ALIVE_COPY['workspace-data'].label}</strong> picks the stop
            point per kind of wait from this workspace's last 30 days. A kind with fewer
            than 50 waits keeps the default.
          </p>
          <p>
            <strong>{CACHE_KEEP_ALIVE_COPY.off.label}</strong> sends no pings.
          </p>
        </Explainer>
      </div>
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Claude Code</div>
            <div class="list-row-details list-row-details-prose">
              Codex threads are unaffected: their cache cannot be kept warm this way.
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
