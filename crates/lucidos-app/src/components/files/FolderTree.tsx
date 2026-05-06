import type { ComponentChildren } from 'preact';
import { artifacts, expandedFolders } from '../../store/store';
import { toggleFolder, buildFolderTree, openFilePreview } from '../../store/actions/artifacts';
import type { FolderNode } from '../../store/actions/artifacts';
import { getEmojiForFile } from '../../utils/fileIcons';
import { loadedOr } from '../../store/types';

type FileEntry = { name: string; path: string };

export function FolderTree() {
  const paths = loadedOr(artifacts.value, []);
  const tree = buildFolderTree(paths);

  return (
    <div class="folder-tree">
      <TreeNode
        node={tree}
        indent={0}
        isExpanded={(path) => expandedFolders.value.has(path)}
        onToggle={toggleFolder}
        onFileClick={openFilePreview}
      />
    </div>
  );
}

export function TreeNode({
  node,
  indent,
  isExpanded,
  onToggle,
  onFileClick,
  folderExtra,
  fileExtra,
  fileClass,
}: {
  node: FolderNode;
  indent: number;
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
          <div key={folderPath} class="folder-item" style={{ paddingLeft: `${indent}rem` }}>
            <div class="folder-header" onClick={() => onToggle(folderPath)}>
              <span class="folder-arrow">{expanded ? '\u25BC' : '\u25B6'}</span>
              <span class="folder-icon">📁</span>
              <span class="folder-name">{folderName}</span>
              <span class="folder-count">({childCount})</span>
              {folderExtra?.(folder)}
            </div>
            {expanded && (
              <div class="folder-contents">
                <TreeNode
                  node={folder}
                  indent={indent + 1}
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

      {files.map((file) => {
        const emoji = getEmojiForFile(file.path);
        return (
          <div
            key={file.path}
            class={`file-item ${fileClass?.(file) ?? ''}`}
            style={{ paddingLeft: `${indent + 1.25}rem` }}
            onClick={() => onFileClick(file.path)}
          >
            <span class="file-icon">{emoji}</span>
            <span class="file-name">{file.name}</span>
            {fileExtra?.(file)}
          </div>
        );
      })}
    </>
  );
}
