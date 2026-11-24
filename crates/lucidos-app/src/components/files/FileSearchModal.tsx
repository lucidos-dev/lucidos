import { useState, useRef, useEffect } from 'preact/hooks';
import {
  artifacts, fileSearchOpen, fileSearchAnchor, activeMenuItem,
  repoFiles, repoSource, repoDiff, changes,
} from '../../store/store';
import { openFilePreview } from '../../store/actions/artifacts';
import { openRepoFilePreview } from '../../store/actions/repositories';
import { FileTypeIcon } from '../../utils/fileIcons';
import {
  collectSearchResults, filterSearchResults, openableChangeFiles, visibleSearchResults,
  type FileSearchResult,
} from './fileSearch';
import { closeFileSearch } from './fileSearchActions';
import { changeBadgeLabel } from './changeBadge';
import { loadedOr } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText, useSkeleton } from '../shared/Skeleton';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { paneUnder, usePaneCentre } from '../../hooks/usePaneCentre';
import { CloseIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';

function sourceBadgeLabel(source: FileSearchResult['source']): string {
  return source === 'workspace' ? 'W' : source === 'repo' ? 'R' : 'C';
}

function SearchResultRow({ result, selected = false, showBadge = false, onHover, onPick }: {
  result?: FileSearchResult;
  selected?: boolean;
  showBadge?: boolean;
  onHover?: () => void;
  onPick?: () => void;
}) {
  if (useSkeleton() || !result) {
    return (
      <div class="file-search-result" aria-hidden="true">
        <SkBlock w="1rem" h="1rem" round />
        <span class="file-search-result-info">
          <SkText class="file-search-result-name" w="8rem" />
          <SkText class="file-search-result-path" w="12rem" />
        </span>
      </div>
    );
  }
  const name = result.path.split('/').pop() || result.path;
  const dir = result.path.includes('/') ? result.path.substring(0, result.path.lastIndexOf('/')) : '';
  return (
    <button
      class={`file-search-result${selected ? ' selected' : ''}`}
      onMouseEnter={onHover}
      onClick={onPick}
    >
      <FileTypeIcon path={result.path} className="file-search-result-icon" />
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
}

function FileSearchPanel() {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);

  const active = activeMenuItem.value;
  const openedOver = useRef(active);
  useEffect(() => {
    if (active !== openedOver.current) closeFileSearch();
  }, [active]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    if (selectedIndex >= 0 && resultsRef.current) {
      const content = resultsRef.current.querySelector('.loading-fade-content');
      const el = content?.children[selectedIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  const isRepo = repoSource.value !== null;
  const primarySource = isRepo ? repoFiles.value : artifacts.value;
  const ccChanges = openableChangeFiles(loadedOr(changes.value, []), isRepo);
  const ccChangesFailed = changes.value.status === 'failed';
  const anyLoaded = primarySource.status === 'loaded' || ccChanges.length > 0;
  const primaryPending =
    primarySource.status === 'loading' || primarySource.status === 'not-loaded';
  const failed =
    primarySource.status === 'failed' || (ccChangesFailed && !anyLoaded && !primaryPending);
  const showLoading = useDelayedFlag(!anyLoaded && !failed);

  const workspacePaths = isRepo ? [] : loadedOr(artifacts.value, []);
  const repoPaths = isRepo ? loadedOr(repoFiles.value, []) : [];
  const diffFiles = isRepo && repoDiff.value.status === 'loaded'
    ? repoDiff.value.data.files.map(f => ({ path: f.path, status: f.status }))
    : [];
  const ccChangeFiles = ccChanges.flatMap(c => c.files.map(f => ({ path: f })));

  const allResults = collectSearchResults(workspacePaths, repoPaths, diffFiles, ccChangeFiles);
  const filtered = filterSearchResults(allResults, query);
  const { shown, hidden } = visibleSearchResults(filtered);
  const showBadge = allResults.some(r => r.source !== allResults[0]?.source);

  const selectResult = (result: FileSearchResult) => {
    closeFileSearch();
    if (result.source === 'workspace') {
      openFilePreview(result.path);
    } else if (result.source === 'repo' || result.source === 'change') {
      openRepoFilePreview(result.path, result.changeStatus ? 'diff' : 'file');
    }
  };

  const closeTouchEnd = (e: TouchEvent) => { e.preventDefault(); closeFileSearch(); };
  const closeBtn = (
    <button class="icon-btn surface-close file-search-close" onTouchEnd={closeTouchEnd} onClick={closeFileSearch} aria-label="Close search" data-tooltip="Close search">
      <CloseIcon />
    </button>
  );

  return (
    <>
      <div class="surface-head file-search-header">
        <SearchField
          class="file-search-field"
          inputRef={inputRef}
          inputClass="file-search-input"
          data-role="file-search-input"
          placeholder="Search files…"
          value={query}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            setSelectedIndex(-1);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Escape') closeFileSearch();
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              setSelectedIndex(i => Math.min(i + 1, shown.length - 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              setSelectedIndex(i => Math.max(i - 1, -1));
            } else if (e.key === 'Enter' && shown.length > 0) {
              e.preventDefault();
              const idx = selectedIndex >= 0 ? selectedIndex : 0;
              selectResult(shown[idx]);
            }
          }}
        />
        {closeBtn}
      </div>
      <div class="file-search-results" ref={resultsRef}>
        {failed && !anyLoaded ? (
          <div class="file-search-empty error-text">Failed to load files</div>
        ) : (
          <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf count={6} row={() => <SearchResultRow />} />}>
            {anyLoaded && (shown.length === 0 ? (
              <div class="file-search-empty">No matching files</div>
            ) : (
              <>
                {shown.map((result, index) => (
                  <SearchResultRow
                    key={`${result.source}:${result.path}`}
                    result={result}
                    selected={index === selectedIndex}
                    showBadge={showBadge}
                    onHover={() => setSelectedIndex(index)}
                    onPick={() => selectResult(result)}
                  />
                ))}
                {hidden > 0 && (
                  <div class="file-search-more">
                    {hidden} more {hidden === 1 ? 'file' : 'files'}. Type to narrow the list.
                  </div>
                )}
              </>
            ))}
          </LoadingFade>
        )}
      </div>
    </>
  );
}

export function FileSearchModal() {
  const open = fileSearchOpen.value;
  const paneCentre = usePaneCentre(open ? paneUnder(fileSearchAnchor.value) ?? 'canvas' : undefined);
  return (
    <Overlay
      open={open}
      onClose={closeFileSearch}
      anchor={fileSearchAnchor.value}
      overlayClass="file-search-overlay"
      panelClass="surface surface-raised surface-pane-centred file-search-modal"
      panelStyle={paneCentre}
      keepMounted
      hiddenClass="file-search-closed"
    >
      <FileSearchPanel />
    </Overlay>
  );
}
