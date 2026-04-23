import { useState, useEffect } from 'preact/hooks';
import { artifactRevision, filePreviewSource, popupImageSrc } from '../../store/store';
import { cognos } from '@cognos/sdk';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { syntaxHighlightJson, syntaxHighlightCode, CODE_EXTS } from '../../utils/syntaxHighlight';
import { renderCsvTable } from '../../utils/csv';
import { SlidesPreview } from './SlidesPreview';
import { isMobile } from '../../utils/viewport';

const imageExts = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp'];
// .ogg is treated as audio (Vorbis/Opus is by far the most common modern usage);
// .ogv is the video variant. Listing 'ogg' in both video and audio caused
// double-render of <video> and <audio> for the same file.
const videoExts = ['mp4', 'webm', 'ogv', 'mov'];
const audioExts = ['mp3', 'wav', 'ogg', 'flac', 'm4a'];
const textExts = [
  'txt', 'md', 'json', 'csv', 'js', 'ts', 'jsx', 'tsx', 'css', 'html', 'xml',
  'py', 'rb', 'go', 'rs', 'java', 'kt', 'kts', 'c', 'cpp', 'h', 'sh', 'bash', 'zsh',
  'yaml', 'yml', 'toml', 'ini', 'cfg', 'conf', 'log', 'sql', 'graphql',
  'vue', 'svelte', 'slides',
];

/** Extensions that have a rendered view and can toggle to source. */
export const RENDERABLE_EXTS = ['md', 'html', 'htm', 'csv', 'svg', 'slides'];

interface Props {
  path: string;
}

export function FilePreviewInline({ path }: Props) {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  const rev = artifactRevision.value;
  const base = cognos.data.url(path);
  const url = rev ? `${base}?v=${rev}` : base;
  const sourceMode = filePreviewSource.value && RENDERABLE_EXTS.includes(ext);

  return (
    <div class="file-preview-inline">
      <div class="file-preview-content">
        {imageExts.includes(ext) && !(ext === 'svg' && sourceMode) && <img src={url} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) popupImageSrc.value = url; }} />}
        {ext === 'pdf' && <iframe src={url} style="width:100%;height:100%;border:none;" />}
        {videoExts.includes(ext) && <video src={url} controls style="max-width:100%;max-height:100%;" />}
        {audioExts.includes(ext) && <audio src={url} controls style="width:100%;" />}
        {(textExts.includes(ext) || (ext === 'svg' && sourceMode)) && <TextContent ext={ext} url={url} sourceMode={sourceMode} />}
        {!imageExts.includes(ext) && ext !== 'pdf' && !videoExts.includes(ext) && !audioExts.includes(ext) && !textExts.includes(ext) && (
          <div class="empty-state">
            <p>Preview not available for <strong>.{ext}</strong> files</p>
            <a href={url} download>Download file</a>
          </div>
        )}
      </div>
    </div>
  );
}

function TextContent({ ext, url, sourceMode }: { ext: string; url: string; sourceMode: boolean }) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setContent(null);
    setError(null);
    fetch(url).then(r => {
      if (!r.ok) throw new Error(`${r.status}`);
      return r.text();
    }).then(setContent).catch(e => setError(e.message));
  }, [url]);

  if (error) return <div class="empty-state" style="color:var(--accent-red)">Failed to load: {error}</div>;
  if (content === null) return <div class="loading-spinner" />;

  if (sourceMode) {
    const lang = ext === 'md' ? 'markdown' : ext === 'csv' ? 'text' : ext === 'svg' ? 'xml' : 'html';
    if (lang === 'text') return <pre class="file-preview-code">{content}</pre>;
    return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightCode(content, lang) }} />;
  }

  if (ext === 'html' || ext === 'htm') return <iframe srcDoc={content} style="width:100%;height:100%;border:none;background:#fff;" />;
  if (ext === 'md') return <div class="response-content markdown-content" dangerouslySetInnerHTML={{ __html: renderMarkdown(content) }} />;
  if (ext === 'json') {
    try {
      const formatted = JSON.stringify(JSON.parse(content), null, 2);
      return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightJson(formatted) }} />;
    } catch {
      return <pre class="file-preview-code">{content}</pre>;
    }
  }
  if (ext === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderCsvTable(content) }} />;
  if (ext === 'slides') return <SlidesPreview content={content} />;
  if (CODE_EXTS.includes(ext)) return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightCode(content, ext) }} />;
  return <pre class="file-preview-code">{content}</pre>;
}
