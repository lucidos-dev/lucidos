import { useState, useRef, useEffect } from 'preact/hooks';
import {
  artifacts, fileSearchOpen, fileSearchAnchor, activeMenuItem,
  repoFiles, repoSource, repoDiff, changes,
} from '../../store/store';
import { openFilePreview } from '../../store/actions/artifacts';
import { openRepoFilePreview } from '../../store/actions/repositories';
import { getEmojiForFile } from '../../utils/fileIcons';
import {
  collectSearchResults, filterSearchResults, type FileSearchResult,
} from './fileSearch';
import { closeFileSearch } from './fileSearchActions';
import { changeBadgeLabel } from './changeBadge';
import { loadedOr } from '../../store/types';
import { Overlay } from '../shared/Overlay';

function sourceBadgeLabel(source: FileSearchResult['source']): string {
  return source === 'workspace' ? 'W' : source === 'repo' ? 'R' : 'C';
}

/** The open-state body. Mounted by `<Overlay>` only while the modal is open, so
 *  the 4k+ result nodes (and their signal subscriptions) and the search compute
 *  don't run on every `visualViewport.resize` while closed — the same reason the
 *  overlay element itself stays mounted (hidden) rather than unmounting. */
function FileSearchPanel() {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);

  const active = activeMenuItem.value;
  useEffect(() => {
    if (active !== 'files') closeFileSearch();
  }, [active]);

  // Auto-focus on mount (the modal only mounts when open). iOS keeps the
  // keyboard from openFileSearch's gesture-stack focus once the real input lands.
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    if (selectedIndex >= 0 && resultsRef.current) {
      const el = resultsRef.current.children[selectedIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  const isRepo = repoSource.value !== null;
  const primarySource = isRepo ? repoFiles.value : artifacts.value;
  // CC change files contribute when `loaded`. A `failed` changes signal
  // bubbles up to the modal's failed state only when no other source has
  // loaded AND the primary source has reached a terminal state — otherwise the
  // modal still functions on what's available, and a still-loading primary
  // shows "Loading..." rather than a premature "Failed to load files" flash.
  const ccChanges = loadedOr(changes.value, []);
  const ccChangesFailed = changes.value.status === 'failed';
  const anyLoaded = primarySource.status === 'loaded' || ccChanges.length > 0;
  const primaryPending =
    primarySource.status === 'loading' || primarySource.status === 'not-loaded';
  const failed =
    primarySource.status === 'failed' || (ccChangesFailed && !anyLoaded && !primaryPending);

  const workspacePaths = isRepo ? [] : loadedOr(artifacts.value, []);
  const repoPaths = isRepo ? loadedOr(repoFiles.value, []) : [];
  const diffFiles = isRepo && repoDiff.value.status === 'loaded'
    ? repoDiff.value.data.files.map(f => ({ path: f.path, status: f.status }))
    : [];
  const ccChangeFiles = ccChanges.flatMap(c =>
    c.files.map(f => ({ path: f })),
  );

  const allResults = collectSearchResults(workspacePaths, repoPaths, diffFiles, ccChangeFiles);
  const filtered = filterSearchResults(allResults, query);
  const showBadge = allResults.some(r => r.source !== allResults[0]?.source);

  const selectResult = (result: FileSearchResult) => {
    closeFileSearch();
    if (result.source === 'workspace') {
      openFilePreview(result.path);
    } else if (result.source === 'repo' || result.source === 'change') {
      openRepoFilePreview(result.path, result.changeStatus ? 'diff' : 'file');
    }
  };

  // On iOS, when the search input is focused the first tap on a button
  // dismisses focus instead of firing click. Using onTouchEnd bypasses this.
  const closeTouchEnd = (e: TouchEvent) => { e.preventDefault(); closeFileSearch(); };
  const closeBtn = (
    <button class="icon-btn header-icon file-search-close" onTouchEnd={closeTouchEnd} onClick={closeFileSearch} aria-label="Close search">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
        <path d="M4 4l8 8M12 4l-8 8" />
      </svg>
    </button>
  );

  if (!anyLoaded) {
    return (
      <div class="file-search-header">
        <span class="file-search-icon" />
        <span class="file-search-input" style="color: var(--text-muted)">
          {failed ? 'Failed to load files' : 'Loading...'}
        </span>
        {closeBtn}
      </div>
    );
  }

  return (
    <>
      <div class="file-search-header">
        <svg class="file-search-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5L14 14" />
        </svg>
        <input
          ref={inputRef}
          class="file-search-input"
          data-role="file-search-input"
          type="text"
          placeholder="Search files..."
          value={query}
          onInput={(e) => {
            setQuery((e.target as HTMLInputElement).value);
            setSelectedIndex(-1);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Escape') closeFileSearch();
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              setSelectedIndex(i => Math.min(i + 1, filtered.length - 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              setSelectedIndex(i => Math.max(i - 1, -1));
            } else if (e.key === 'Enter' && filtered.length > 0) {
              e.preventDefault();
              const idx = selectedIndex >= 0 ? selectedIndex : 0;
              selectResult(filtered[idx]);
            }
          }}
        />
        {closeBtn}
      </div>
      <div class="file-search-results" ref={resultsRef}>
        {filtered.length === 0 ? (
          <div class="file-search-empty">No matching files</div>
        ) : (
          filtered.map((result, index) => {
            const name = result.path.split('/').pop() || result.path;
            const dir = result.path.includes('/') ? result.path.substring(0, result.path.lastIndexOf('/')) : '';
            const emoji = getEmojiForFile(result.path);
            return (
              <button
                key={`${result.source}:${result.path}`}
                class={`file-search-result${index === selectedIndex ? ' selected' : ''}`}
                onMouseEnter={() => setSelectedIndex(index)}
                onClick={() => selectResult(result)}
              >
                <span class="file-search-result-icon">{emoji}</span>
                <span class="file-search-result-info">
                  <span class="file-search-result-name">{name}</span>
                  {dir && <span class="file-search-result-path">{dir}</span>}
                </span>
                {showBadge && (
                  <span class={`file-search-source-badge file-search-source-${result.source}`}>
                    {sourceBadgeLabel(result.source)}
                  </span>
                )}
                {result.changeStatus && (
                  <span class={`change-badge change-badge-${result.changeStatus}`}>
                    {changeBadgeLabel(result.changeStatus)}
                  </span>
                )}
              </button>
            );
          })
        )}
      </div>
    </>
  );
}

export function FileSearchModal() {
  // keepMounted: the overlay div is NEVER removed from the DOM — iOS Safari
  // PWA's compositor leaves ghost pixels when a fixed-position layer is removed
  // from the layer tree, so closing toggles `.file-search-closed` instead. The
  // heavy body (FileSearchPanel) only mounts while open. Same pattern as
  // SearchEverywhere. Contract (dismiss + swallow + anchor + Escape) lives in
  // <Overlay>.
  return (
    <Overlay
      open={fileSearchOpen.value}
      onClose={closeFileSearch}
      anchor={fileSearchAnchor.value}
      overlayClass="file-search-overlay"
      panelClass="file-search-modal"
      keepMounted
      hiddenClass="file-search-closed"
    >
      <FileSearchPanel />
    </Overlay>
  );
}
