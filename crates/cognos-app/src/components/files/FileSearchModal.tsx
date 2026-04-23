import { useState, useRef, useEffect } from 'preact/hooks';
import {
  artifacts, fileSearchOpen, activeMenuItem,
  repoFiles, repoSource, repoDiff, changes,
  encodeRepoPath, panelOverlay, selectedLines,
} from '../../store/store';
import { openFilePreview } from '../../store/actions/artifacts';
import { navigateToPane } from '../../store/actions/pane';
import { pushNavState } from '../../store/actions/navigation';
import { getEmojiForFile } from '../../utils/fileIcons';
import { isMobile } from '../../utils/viewport';
import {
  collectSearchResults, filterSearchResults, type FileSearchResult,
} from './fileSearch';
import { changeBadgeLabel } from './changeBadge';
import { loadedOr } from '../../store/types';

function sourceBadgeLabel(source: FileSearchResult['source']): string {
  return source === 'workspace' ? 'W' : source === 'repo' ? 'R' : 'C';
}

/** Open the file search modal and focus the input within the current call stack.
 *  Must be called synchronously from a user gesture (touch/click) for iOS to
 *  open the keyboard. Preact signals render synchronously, so after setting
 *  fileSearchOpen the input is visible and focusable immediately. */
export function openFileSearch(): void {
  fileSearchOpen.value = true;
  const input = document.querySelector<HTMLInputElement>('[data-role="file-search-input"]');
  if (input) input.focus({ preventScroll: true });
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

  // All signal reads happen unconditionally so the full DOM tree is always
  // rendered. On close we only toggle a CSS class — zero DOM mutations.
  // This prevents iOS Safari PWA compositor ghost pixels.
  const workspaceLoaded = artifacts.value.status === 'loaded';
  const anyLoaded = workspaceLoaded
    || repoFiles.value.status === 'loaded'
    || changes.value.length > 0;
  const failed = artifacts.value.status === 'failed';

  const isRepo = repoSource.value !== null;
  const workspacePaths = isRepo ? [] : loadedOr(artifacts.value, []);
  const repoPaths = isRepo ? loadedOr(repoFiles.value, []) : [];
  const diffFiles = isRepo && repoDiff.value.status === 'loaded'
    ? repoDiff.value.data.files.map(f => ({ path: f.path, status: f.status }))
    : [];
  const ccChangeFiles = changes.value.flatMap(c =>
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
      const repoId = repoSource.value;
      if (repoId) {
        selectedLines.value = null;
        panelOverlay.value = {
          type: 'file-preview',
          path: encodeRepoPath(repoId, result.changeStatus ? 'diff' : 'file', result.path),
        };
        if (isMobile()) navigateToPane('content');
        pushNavState();
      }
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
      class={`modal-overlay file-search-overlay${isOpen ? '' : ' file-search-closed'}`}
      onTouchEnd={isOpen ? (e: TouchEvent) => {
        if (e.target === e.currentTarget) { e.preventDefault(); close(); }
      } : undefined}
      onClick={isOpen ? (e: MouseEvent) => {
        if (e.target === e.currentTarget) close();
      } : undefined}
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
