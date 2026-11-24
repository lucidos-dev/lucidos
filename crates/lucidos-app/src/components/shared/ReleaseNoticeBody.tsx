import { appsList } from '../../store/store';
import { loadedOr } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { handleMarkdownLinkClick } from './markdownLinkClick';

export function ReleaseNoticeBody({ body, onNavigate }: { body: string; onNavigate?: () => void }) {
  return (
    <div
      class="markdown-content release-notice-body authored-html"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(body) }}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest('a[href]')) {
          onNavigate?.();
        }
        handleMarkdownLinkClick(e, loadedOr(appsList.value, []), 'a release notice');
      }}
    />
  );
}
