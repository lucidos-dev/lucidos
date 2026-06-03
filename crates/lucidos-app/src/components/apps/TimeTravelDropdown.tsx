import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { currentApp, appCommit } from '../../store/store';
import { getAppVersions, type AppVersion } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';

const PAGE_SIZE = 10;

export function TimeTravelDropdown() {
  const app = currentApp.value;
  const [open, setOpen] = useState(false);
  const [versionsLoadable, setVersionsLoadable] = useState<Loadable<AppVersion[]>>({ status: 'not-loaded' });
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const showLoading = useDelayedLoading(versionsLoadable);
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const appId = app?.id ?? null;

  useDismissOnOutside(open, ref, null, () => setOpen(false));

  // Clicking into an iframe steals focus without firing pointerdown in our
  // document, so the dismiss hook can't see it — the blur listener catches it.
  useEffect(() => {
    if (!open) return;
    function handleBlur() {
      setOpen(false);
    }
    window.addEventListener('blur', handleBlur);
    return () => window.removeEventListener('blur', handleBlur);
  }, [open]);

  // Reset cached versions when the visible app changes. Previously this lived
  // in the render body gated by a useRef — preact lint flags that as a
  // side-effect-in-render gotcha; this effect is the canonical place. Guarded
  // so the initial mount (state already at the reset values) doesn't bounce
  // through three no-op setState calls.
  useEffect(() => {
    setVersionsLoadable(prev => prev.status === 'not-loaded' ? prev : { status: 'not-loaded' });
    setHasMore(prev => prev ? false : prev);
    setLoadingMore(prev => prev ? false : prev);
  }, [appId]);

  if (!app || appId === null) return null;
  // Local rebind so closures below (loadPage) see the narrowed string type
  // — TS does not propagate the early-return narrowing of `appId` into
  // inner functions defined after this point.
  const resolvedAppId = appId;
  const loadedVersions = versionsLoadable.status === 'loaded' ? versionsLoadable.data : null;

  async function loadPage(skip: number, append: boolean) {
    if (append) {
      setLoadingMore(true);
    } else {
      setVersionsLoadable({ status: 'loading' });
    }
    try {
      const page = await getAppVersions(resolvedAppId, PAGE_SIZE, skip);
      if (append) {
        setVersionsLoadable(prev => ({
          status: 'loaded',
          data: prev.status === 'loaded' ? [...prev.data, ...page.versions] : page.versions,
        }));
      } else {
        setVersionsLoadable({ status: 'loaded', data: page.versions });
      }
      setHasMore(page.has_more);
    } catch (err) {
      setVersionsLoadable(toFailed(err));
    } finally {
      setLoadingMore(false);
    }
  }

  function toggle() {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    if (loadedVersions && loadedVersions.length > 0) return;
    void loadPage(0, false);
  }

  const handleScroll = useCallback(() => {
    const menu = menuRef.current;
    if (!menu || loadingMore || !hasMore || !loadedVersions) return;
    // Load more when scrolled within 2rem (32px) of the bottom
    if (menu.scrollTop + menu.clientHeight >= menu.scrollHeight - 32) {
      void loadPage(loadedVersions.length, true);
    }
  }, [loadingMore, hasMore, loadedVersions]);

  function selectVersion(commit: string | null) {
    appCommit.value = commit;
    setOpen(false);
  }

  const activeCommit = appCommit.value;

  return (
    <div class="time-travel" ref={ref}>
      <button
        class={`icon-btn header-icon${activeCommit ? ' filter-active' : ''}`}
        onClick={toggle}
        data-tooltip="Time travel: browse older version of this ui"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10" />
          <polyline points="12 6 12 12 16 14" />
        </svg>
      </button>
      {open && (
        <div class="time-travel-menu" ref={menuRef} onScroll={handleScroll}>
          <div
            class={`time-travel-item${!activeCommit ? ' active' : ''}`}
            onClick={() => selectVersion(null)}
          >
            <span class="time-travel-label">Latest</span>
          </div>
          {versionsLoadable.status === 'loading' && showLoading && (
            <div class="time-travel-item time-travel-loading">Loading...</div>
          )}
          {versionsLoadable.status === 'failed' && (
            <div class="time-travel-item time-travel-error">{versionsLoadable.error}</div>
          )}
          {loadedVersions && loadedVersions.map((v) => (
            <div
              key={v.commit}
              class={`time-travel-item${activeCommit === v.commit ? ' active' : ''}`}
              onClick={() => selectVersion(v.commit)}
              data-tooltip={v.commit.slice(0, 8)}
            >
              <span class="time-travel-label">{v.message || v.commit.slice(0, 8)}</span>
              <span class="time-travel-date">{formatTimeAgo(new Date(v.timestamp * 1000))}</span>
            </div>
          ))}
          {loadingMore && <div class="time-travel-item time-travel-loading">Loading more...</div>}
          {loadedVersions && loadedVersions.length === 0 && (
            <div class="time-travel-item time-travel-empty">No history</div>
          )}
        </div>
      )}
    </div>
  );
}
