import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { currentApp, appCommit } from '../../store/store';
import { getAppVersions, type AppVersion } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import { errorDetail } from '../../utils/errorDetail';

const PAGE_SIZE = 10;

export function TimeTravelDropdown() {
  const app = currentApp.value;
  const [open, setOpen] = useState(false);
  const [versions, setVersions] = useState<AppVersion[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const lastAppId = useRef<string | null>(null);

  useEffect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    // Clicking inside an iframe steals focus from the parent window
    function handleBlur() {
      setOpen(false);
    }
    document.addEventListener('click', handleClick);
    window.addEventListener('blur', handleBlur);
    return () => {
      document.removeEventListener('click', handleClick);
      window.removeEventListener('blur', handleBlur);
    };
  }, [open]);

  if (!app) return null;
  const appId = app.id;

  // Reset cached versions when app changes
  if (lastAppId.current !== appId) {
    lastAppId.current = appId;
    if (versions.length > 0) setVersions([]);
    if (error) setError(null);
    setHasMore(false);
    setLoading(false);
    setLoadingMore(false);
  }

  async function loadPage(skip: number, append: boolean) {
    if (append) {
      setLoadingMore(true);
    } else {
      setLoading(true);
    }
    setError(null);
    try {
      const page = await getAppVersions(appId, PAGE_SIZE, skip);
      if (append) {
        setVersions(prev => [...prev, ...page.versions]);
      } else {
        setVersions(page.versions);
      }
      setHasMore(page.has_more);
    } catch (err) {
      setError(errorDetail(err));
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }

  async function toggle() {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    if (versions.length > 0) return;
    loadPage(0, false);
  }

  const handleScroll = useCallback(() => {
    const menu = menuRef.current;
    if (!menu || loadingMore || !hasMore) return;
    // Load more when scrolled within 2rem (32px) of the bottom
    if (menu.scrollTop + menu.clientHeight >= menu.scrollHeight - 32) {
      loadPage(versions.length, true);
    }
  }, [loadingMore, hasMore, versions.length]);

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
          {loading && <div class="time-travel-item time-travel-loading">Loading...</div>}
          {error && <div class="time-travel-item time-travel-error">{error}</div>}
          {versions.map((v) => (
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
          {!loading && !error && versions.length === 0 && (
            <div class="time-travel-item time-travel-empty">No history</div>
          )}
        </div>
      )}
    </div>
  );
}
