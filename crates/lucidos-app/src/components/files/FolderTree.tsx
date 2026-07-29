import type { ComponentChildren } from 'preact';
import { artifacts, expandedFolders } from '../../store/store';
import { toggleFolder, buildFolderTree, openFilePreview } from '../../store/actions/artifacts';
import type { FolderNode } from '../../store/actions/artifacts';
import { FileTypeIcon, FolderIcon } from '../../utils/fileIcons';
import { loadedOr } from '../../store/types';
import { SkText, SkBlock } from '../shared/Skeleton';

type FileEntry = { name: string; path: string };

/** One placeholder tree row mirroring the real `.folder-item` / `.file-item`
 *  node markup (icon box + name) with shimmer leaves, indented like a real node.
 *  The real tree gets its offset from being nested in `.folder-contents`, but the
 *  skeleton's rows are flat siblings — so wrap the row in `depth` such shells to
 *  borrow the same per-level offset rather than restating it as a number here.
 *  Used only inside a SkeletonProvider (via {@link folderTreeSkeletonRow}). */
function TreeRowSkeleton({ kind, depth }: { kind: 'folder' | 'file'; depth: number }) {
  let row =
    kind === 'folder' ? (
      <div class="folder-item">
        <div class="folder-header">
          <SkBlock w="0.6rem" h="0.6rem" round />
          <SkBlock w="1rem" h="1rem" round />
          <SkText class="folder-name" w="7rem" />
        </div>
      </div>
    ) : (
      <div class="file-item tree-file-item">
        <SkBlock w="1rem" h="1rem" round />
        <SkText class="file-name" w="9rem" />
      </div>
    );
  for (let i = 0; i < depth; i++) row = <div class="folder-contents">{row}</div>;
  return row;
}

/** A representative folder/file shape, cycled by row index so the tree skeleton
 *  shows real nesting (folders + indented files) instead of a flat run of bars —
 *  the tree's true shape is unknown before load, so this stands in for it. */
const TREE_SKELETON_PATTERN: { kind: 'folder' | 'file'; depth: number }[] = [
  { kind: 'folder', depth: 0 },
  { kind: 'file', depth: 1 },
  { kind: 'file', depth: 1 },
  { kind: 'folder', depth: 0 },
  { kind: 'file', depth: 1 },
  { kind: 'file', depth: 0 },
];

/** Row thunk for `<ListSkeletonOf row={folderTreeSkeletonRow} />` — mirrors the
 *  FolderTree / TreeNode layout while artifacts (or the repo file tree) load. */
export function folderTreeSkeletonRow(i: number) {
  const node = TREE_SKELETON_PATTERN[i % TREE_SKELETON_PATTERN.length];
  return <TreeRowSkeleton kind={node.kind} depth={node.depth} />;
}

export function FolderTree() {
  const paths = loadedOr(artifacts.value, []);
  const tree = buildFolderTree(paths);

  return (
    <div class="folder-tree">
      <TreeNode
        node={tree}
        isExpanded={(path) => expandedFolders.value.has(path)}
        onToggle={toggleFolder}
        onFileClick={openFilePreview}
      />
    </div>
  );
}

/** Renders one level of the tree. Indentation is NOT computed here: a level's
 *  rows are nested inside the parent's `.folder-contents`, which carries the
 *  single per-level offset (see `components.css`). Reintroducing a per-row
 *  offset on top of that DOM nesting is what made each level indent by the
 *  running total instead of one unit. */
export function TreeNode({
  node,
  isExpanded,
  onToggle,
  onFileClick,
  folderExtra,
  fileExtra,
  fileClass,
}: {
  node: FolderNode;
  isExpanded: (path: string) => boolean;
  onToggle: (path: string) => void;
  onFileClick: (path: string) => void;
  folderExtra?: (folder: FolderNode) => ComponentChildren;
  fileExtra?: (file: FileEntry) => ComponentChildren;
  fileClass?: (file: FileEntry) => string;
}) {
  const folderNames = Object.keys(node.children).sort();
  const files = [...node.files].sort((a, b) => a.name.localeCompare(b.name));

  return (
    <>
      {folderNames.map((folderName) => {
        const folder = node.children[folderName];
        const folderPath = folder.path!;
        const expanded = isExpanded(folderPath);
        const childCount =
          Object.keys(folder.children).length + folder.files.length;

        return (
          <div key={folderPath} class="folder-item">
            <div class="folder-header" onClick={() => onToggle(folderPath)}>
              <span class="folder-arrow">{expanded ? '\u25BC' : '\u25B6'}</span>
              <FolderIcon className="folder-icon" />
              <span class="folder-name">{folderName}</span>
              <span class="folder-count">({childCount})</span>
              {folderExtra?.(folder)}
            </div>
            {expanded && (
              <div class="folder-contents">
                <TreeNode
                  node={folder}
                  isExpanded={isExpanded}
                  onToggle={onToggle}
                  onFileClick={onFileClick}
                  folderExtra={folderExtra}
                  fileExtra={fileExtra}
                  fileClass={fileClass}
                />
              </div>
            )}
          </div>
        );
      })}

      {files.map((file) => (
        <div
          key={file.path}
          class={`file-item tree-file-item ${fileClass?.(file) ?? ''}`}
          onClick={() => onFileClick(file.path)}
        >
          <FileTypeIcon path={file.path} className="file-icon" />
          <span class="file-name">{file.name}</span>
          {fileExtra?.(file)}
        </div>
      ))}
    </>
  );
}
