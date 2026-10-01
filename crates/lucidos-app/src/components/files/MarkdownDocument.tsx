import { useMemo } from 'preact/hooks';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { splitFrontmatter, type Frontmatter } from '../../utils/frontmatter';

/** A markdown file as a document: its frontmatter as a properties card, then
 *  the rendered body. The raw text stays in the editor and the Source view. */
export function MarkdownDocument({ content, onClick }: {
  content: string;
  onClick?: (e: MouseEvent) => void;
}) {
  const { frontmatter, html } = useMemo(() => {
    const split = splitFrontmatter(content);
    return { frontmatter: split.frontmatter, html: renderMarkdown(split.body) };
  }, [content]);
  return (
    <>
      {frontmatter && <FrontmatterCard frontmatter={frontmatter} />}
      <div
        class="response-content markdown-content"
        onClick={onClick ? (e) => onClick(e as unknown as MouseEvent) : undefined}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </>
  );
}

export function FrontmatterCard({ frontmatter }: { frontmatter: Frontmatter }) {
  if (frontmatter.kind === 'raw') {
    return <pre class="frontmatter-card frontmatter-card-raw" aria-label="Properties">{frontmatter.text}</pre>;
  }
  return (
    <dl class="frontmatter-card" aria-label="Properties">
      {frontmatter.fields.map(([key, value], row) => (
        <div class="frontmatter-row" key={row}>
          <dt>{key}</dt>
          {Array.isArray(value)
            ? <dd class="frontmatter-chips">{value.map((item, i) => <span class="frontmatter-chip" key={i}>{item}</span>)}</dd>
            : <dd>{value}</dd>}
        </div>
      ))}
    </dl>
  );
}
