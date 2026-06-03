import { useState, useEffect, useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { browseDirectories, type BrowseResult } from '../../api/client';
import { toFailed, type Loadable } from '../../store/types';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';

interface DirectoryPickerProps {
  onSelect: (path: string) => void;
  onCancel: () => void;
}

/** Inner children of `.dir-picker-list` — branches on all four Loadable
 *  states so loading / failed are visually distinct from loaded-empty. */
export function directoryPickerBody({
  data,
  currentPath,
  selectedIndex,
  onGoUp,
  onSelectDir,
  onHoverIndex,
}: {
  data: Loadable<BrowseResult>;
  currentPath: string;
  selectedIndex: number;
  onGoUp: () => void;
  onSelectDir: (dir: string) => void;
  onHoverIndex: (idx: number) => void;
}): ComponentChildren {
  if (data.status === 'not-loaded' || data.status === 'loading') {
    return (
      <div class="dir-picker-empty loading-skeleton" data-state="loading">
        Loading...
      </div>
    );
  }
  if (data.status === 'failed') {
    return (
      <div class="dir-picker-empty dir-picker-error" data-state="failed">
        {data.error}
      </div>
    );
  }
  const dirs = data.data.directories;
  const showParent = currentPath !== '/';
  if (dirs.length === 0 && !showParent) {
    return <div class="dir-picker-empty" data-state="empty">No subdirectories</div>;
  }
  return (
    <>
      {showParent && (
        <button
          class={`dir-picker-row${selectedIndex === 0 ? ' selected' : ''}`}
          onClick={onGoUp}
          onMouseEnter={() => onHoverIndex(0)}
        >
          <span class="dir-picker-icon">{'\u{1F4C2}'}</span>
          <span class="dir-picker-name">..</span>
        </button>
      )}
      {dirs.length === 0 && (
        <div class="dir-picker-empty" data-state="empty">No subdirectories</div>
      )}
      {dirs.map((dir, i) => {
        const idx = showParent ? i + 1 : i;
        return (
          <button
            key={dir}
            class={`dir-picker-row${idx === selectedIndex ? ' selected' : ''}`}
            onClick={() => onSelectDir(dir)}
            onMouseEnter={() => onHoverIndex(idx)}
          >
            <span class="dir-picker-icon">{'\u{1F4C1}'}</span>
            <span class="dir-picker-name">{dir}</span>
          </button>
        );
      })}
    </>
  );
}

export function DirectoryPicker({ onSelect, onCancel }: DirectoryPickerProps) {
  const [browsePath, setBrowsePath] = useState<string | undefined>(undefined);
  const [data, setData] = useState<Loadable<BrowseResult>>({ status: 'loading' });
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [manualPath, setManualPath] = useState('');
  const [editingPath, setEditingPath] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useDismissOnOutside(true, panelRef, null, onCancel);

  useEffect(() => {
    setData({ status: 'loading' });
    setSelectedIndex(-1);
    browseDirectories(browsePath)
      .then(result => {
        setData({ status: 'loaded', data: result });
        setManualPath(result.path);
      })
      .catch(e => setData(toFailed(e)));
  }, [browsePath]);

  useEffect(() => {
    if (editingPath && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingPath]);

  const navigateTo = (path: string) => {
    setBrowsePath(path);
    setEditingPath(false);
  };

  const goUp = () => {
    if (data.status !== 'loaded') return;
    const parent = data.data.path.replace(/\/[^/]+\/?$/, '') || '/';
    navigateTo(parent);
  };

  const currentPath = data.status === 'loaded' ? data.data.path : browsePath || '~';
  const segments = (data.status === 'loaded' ? data.data.path : '').split('/').filter(Boolean);
  const isGitRepo = data.status === 'loaded' && data.data.is_git_repo;
  const navMaxIdx = data.status === 'loaded'
    ? data.data.directories.length - 1 + (currentPath !== '/' ? 1 : 0)
    : 0;
  const joinPath = (parent: string, child: string) => parent === '/' ? '/' + child : parent + '/' + child;

  return (
    <div class="confirm-overlay">
      <div class="dir-picker" ref={panelRef}>
        <div class="dir-picker-header">
          <span class="dir-picker-title">Select Directory</span>
          <button class="icon-btn header-icon" onClick={onCancel} aria-label="Close">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>

        <div class="dir-picker-breadcrumb">
          {editingPath ? (
            <input
              ref={inputRef}
              class="dir-picker-path-input"
              value={manualPath}
              onInput={(e) => setManualPath((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') navigateTo(manualPath);
                if (e.key === 'Escape') setEditingPath(false);
              }}
              onBlur={() => setEditingPath(false)}
            />
          ) : (
            <div class="dir-picker-segments" onClick={() => setEditingPath(true)}>
              <span class="dir-picker-segment" onClick={(e) => { e.stopPropagation(); navigateTo('/'); }}>/</span>
              {segments.map((seg, i) => (
                <span key={i}>
                  <span
                    class="dir-picker-segment"
                    onClick={(e) => { e.stopPropagation(); navigateTo('/' + segments.slice(0, i + 1).join('/')); }}
                  >{seg}</span>
                  {i < segments.length - 1 && <span class="dir-picker-sep">/</span>}
                </span>
              ))}
            </div>
          )}
        </div>

        <div class="dir-picker-list" onKeyDown={(e) => {
          if (e.key === 'ArrowDown') { e.preventDefault(); setSelectedIndex(i => Math.min(i + 1, navMaxIdx)); }
          if (e.key === 'ArrowUp') { e.preventDefault(); setSelectedIndex(i => Math.max(i - 1, -1)); }
          if (e.key === 'Enter' && data.status === 'loaded') {
            e.preventDefault();
            if (selectedIndex === 0 && currentPath !== '/') goUp();
            else {
              const dirIdx = currentPath !== '/' ? selectedIndex - 1 : selectedIndex;
              if (dirIdx >= 0) navigateTo(joinPath(data.data.path, data.data.directories[dirIdx]));
            }
          }
          if (e.key === 'Escape') onCancel();
        }} tabIndex={0}>
          {directoryPickerBody({
            data,
            currentPath,
            selectedIndex,
            onGoUp: goUp,
            onSelectDir: (dir) => {
              if (data.status === 'loaded') navigateTo(joinPath(data.data.path, dir));
            },
            onHoverIndex: setSelectedIndex,
          })}
        </div>

        <div class="dir-picker-footer">
          <div class="dir-picker-status">
            {isGitRepo && <span class="dir-picker-git-badge">git repo</span>}
          </div>
          <div class="dir-picker-actions">
            <button class="action-btn" onClick={onCancel}>Cancel</button>
            <button
              class="action-btn action-btn-confirm"
              disabled={data.status !== 'loaded'}
              onClick={() => { if (data.status === 'loaded') onSelect(data.data.path); }}
            >Select</button>
          </div>
        </div>
      </div>
    </div>
  );
}
