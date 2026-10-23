import { appsList } from '../../store/store';
import { loadedOr } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { handleMarkdownLinkClick } from './markdownLinkClick';

/** A *release notice*'s markdown body, with its links routed in the app.
 *  A notice points at a settings page with a link such as
 *  `[Settings > System > Memory](settings:memory)`. */
export function ReleaseNoticeBody({ body }: { body: string }) {
  return (
    <div
      class="markdown-content release-notice-body"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(body) }}
      onClick={(e) => handleMarkdownLinkClick(e, loadedOr(appsList.value, []), 'a release notice')}
    />
  );
}
