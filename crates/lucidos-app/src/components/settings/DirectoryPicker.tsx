import { useState, useEffect, useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { browseDirectories, type BrowseResult } from '../../api/client';
import { toFailed, type Loadable } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { FolderIcon, FolderUpIcon } from '../shared/icons';
import { SurfaceHead } from '../shared/Surface';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';

interface DirectoryPickerProps {
  onSelect: (path: string) => void;
  onCancel: () => void;
}

/** One directory. With no `name`, inside a `SkeletonProvider`, it is the
 *  list's loading placeholder. */
function DirRow({ name, up = false, selected = false, onClick, onMouseEnter }: {
  name?: string;
  up?: boolean;
  selected?: boolean;
  onClick?: () => void;
  onMouseEnter?: () => void;
}) {
  return (
    <button
      class={`dir-picker-row${selected ? ' selected' : ''}`}
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      tabIndex={name === undefined ? -1 : undefined}
    >
      <SkBlock w="var(--icon-size-lg)" h="var(--icon-size-lg)" round>
        <span class="dir-picker-icon">{up ? <FolderUpIcon /> : <FolderIcon />}</span>
      </SkBlock>
      <SkText class="dir-picker-name" w="9rem">{name}</SkText>
    </button>
  );
}

/** Inner children of `.dir-picker-list`. Branches on all four Loadable states
 *  so loading and failed are visually distinct from loaded-empty. */
export function directoryPickerBody({
  data,
  showLoading,
  currentPath,
  selectedIndex,
  onGoUp,
  onSelectDir,
  onHoverIndex,
}: {
  data: Loadable<BrowseResult>;
  showLoading: boolean;
  currentPath: string;
  selectedIndex: number;
  onGoUp: () => void;
  onSelectDir: (dir: string) => void;
  onHoverIndex: (idx: number) => void;
}): ComponentChildren {
  if (data.status === 'failed') {
    return (
      <div class="dir-picker-empty dir-picker-error" data-state="failed">
        {data.error}
      </div>
    );
  }
  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf count={6} row={() => <DirRow />} />}>
      {data.status === 'loaded' && loadedRows(data.data)}
    </LoadingFade>
  );

  function loadedRows({ directories: dirs }: BrowseResult) {
  const showParent = currentPath !== '/';
    if (dirs.length === 0 && !showParent) {
      return <div class="dir-picker-empty" data-state="empty">No subdirectories</div>;
    }
    return (
      <>
        {showParent && (
          <DirRow name=".." up selected={selectedIndex === 0} onClick={onGoUp} onMouseEnter={() => onHoverIndex(0)} />
        )}
        {dirs.length === 0 && (
          <div class="dir-picker-empty" data-state="empty">No subdirectories</div>
        )}
        {dirs.map((dir, i) => {
          const idx = showParent ? i + 1 : i;
          return (
            <DirRow
              key={dir}
              name={dir}
              selected={idx === selectedIndex}
              onClick={() => onSelectDir(dir)}
              onMouseEnter={() => onHoverIndex(idx)}
            />
          );
        })}
      </>
    );
  }
}

export function DirectoryPicker({ onSelect, onCancel }: DirectoryPickerProps) {
  const [browsePath, setBrowsePath] = useState<string | undefined>(undefined);
  const [data, setData] = useState<Loadable<BrowseResult>>({ status: 'loading' });
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [manualPath, setManualPath] = useState('');
  const [editingPath, setEditingPath] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const showLoading = useDelayedLoading(data);

  useEffect(() => {
    // Fast breadcrumb clicks overlap reads. A superseded reply must not land,
    // or Select would return a directory the user already left.
    let cancelled = false;
    setData({ status: 'loading' });
    setSelectedIndex(-1);
    browseDirectories(browsePath)
      .then(result => {
        if (cancelled) return;
        setData({ status: 'loaded', data: result });
        setManualPath(result.path);
      })
      .catch(e => { if (!cancelled) setData(toFailed(e)); });
    return () => { cancelled = true; };
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
    // A centered modal, so it takes `<Overlay>`'s own `.modal-overlay`
    // container. `.dir-picker` places nothing itself, so a wrapper of its own
    // would leave the panel unscrimmed and in flow.
    <Overlay open onClose={onCancel} panelClass="surface surface-raised dir-picker" panelRole="dialog" ariaModal>
      <SurfaceHead title="Select Directory" onClose={onCancel} closeLabel="Close directory picker" />

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
          showLoading,
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
          <button class="action-btn action-btn-secondary" onClick={onCancel}>Cancel</button>
          <button
            class="action-btn action-btn-confirm"
            disabled={data.status !== 'loaded'}
            onClick={() => { if (data.status === 'loaded') onSelect(data.data.path); }}
          >Select</button>
        </div>
      </div>
    </Overlay>
  );
}
