import { useState, useRef, useEffect } from 'preact/hooks';
import {
  artifacts, fileSearchOpen, activeMenuItem,
  repoFiles, repoSource, repoDiff, changes,
} from '../../store/store';
import { openFilePreview } from '../../store/actions/artifacts';
import { openRepoFilePreview } from '../../store/actions/repositories';
import { getEmojiForFile } from '../../utils/fileIcons';
import {
  collectSearchResults, filterSearchResults, type FileSearchResult,
} from './fileSearch';
import { changeBadgeLabel } from './changeBadge';
import { loadedOr } from '../../store/types';

function sourceBadgeLabel(source: FileSearchResult['source']): string {
  return source === 'workspace' ? 'W' : source === 'repo' ? 'R' : 'C';
}

export function FileSearchModal() {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const isOpen = fileSearchOpen.value;

  const close = () => {
    fileSearchOpen.value = false;
    setQuery('');
    setSelectedIndex(-1);
  };

  const active = activeMenuItem.value;
  useEffect(() => {
    if (active !== 'files') close();
  }, [active]);

  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  useEffect(() => {
    if (selectedIndex >= 0 && resultsRef.current) {
      const el = resultsRef.current.children[selectedIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  // Render an empty hidden shell when closed. The 4k+ child nodes (and their
  // signal subscriptions) would otherwise re-layout on every visualViewport.resize
  // (each iOS keyboard show/hide writes --app-height, which propagates through
  // position:fixed; inset:0). Same pattern as SearchEverywhere.tsx.
  if (!isOpen) {
    return <div class="modal-overlay file-search-overlay file-search-closed" aria-hidden="true" />;
  }

  const isRepo = repoSource.value !== null;
  const primarySource = isRepo ? repoFiles.value : artifacts.value;
  // CC change files contribute when `loaded`. A `failed` changes signal
  // bubbles up to the modal's failed state only when no other source has
  // loaded — otherwise the modal still functions on what's available.
  const ccChanges = loadedOr(changes.value, []);
  const ccChangesFailed = changes.value.status === 'failed';
  const anyLoaded = primarySource.status === 'loaded' || ccChanges.length > 0;
  const failed = primarySource.status === 'failed' || (ccChangesFailed && !anyLoaded);

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
    close();
    if (result.source === 'workspace') {
      openFilePreview(result.path);
    } else if (result.source === 'repo' || result.source === 'change') {
      openRepoFilePreview(result.path, result.changeStatus ? 'diff' : 'file');
    }
  };

  // On iOS, when the search input is focused the first tap on a button
  // dismisses focus instead of firing click. Using onTouchEnd bypasses this.
  const closeTouchEnd = (e: TouchEvent) => { e.preventDefault(); close(); };
  const closeBtn = (
    <button class="icon-btn header-icon file-search-close" onTouchEnd={closeTouchEnd} onClick={close} aria-label="Close search">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
        <path d="M4 4l8 8M12 4l-8 8" />
      </svg>
    </button>
  );

  return (
    <div
      class="modal-overlay file-search-overlay"
      onTouchEnd={(e: TouchEvent) => {
        if (e.target === e.currentTarget) { e.preventDefault(); close(); }
      }}
      onClick={(e: MouseEvent) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div class="file-search-modal">
        {!anyLoaded ? (
          <div class="file-search-header">
            <span class="file-search-icon" />
            <span class="file-search-input" style="color: var(--text-muted)">
              {failed ? 'Failed to load files' : 'Loading...'}
            </span>
            {closeBtn}
          </div>
        ) : (
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
                  if (e.key === 'Escape') close();
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
        )}
      </div>
    </div>
  );
}
