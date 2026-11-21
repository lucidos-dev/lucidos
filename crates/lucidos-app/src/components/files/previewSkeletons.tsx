import { LineNumberedCode, type WideLineMode } from './LineNumberedCode';
import { SkText, SkeletonProvider } from '../shared/Skeleton';

/** A source file's loading placeholder: the line-numbered view itself, drawn
 *  under a `SkeletonProvider`. */
export function FileSourceSkeleton({ wideLines }: { wideLines: WideLineMode }) {
  return (
    <SkeletonProvider>
      <LineNumberedCode rows={[]} wideLines={wideLines} />
    </SkeletonProvider>
  );
}

/** Line widths a rendered document shimmers with while it loads. */
const PROSE_LINE_WIDTHS = ['38%', '94%', '88%', '72%', '', '91%', '84%', '56%'];

/** A rendered document's loading placeholder (markdown, HTML, a table): lines
 *  of text in the real `.markdown-content` box, since a document has no row
 *  component to draw from. An empty width is a paragraph break. */
export function ProseSkeleton({ class: cls = 'response-content markdown-content' }: { class?: string }) {
  return (
    <SkeletonProvider>
      <div class={cls} aria-hidden="true">
        {PROSE_LINE_WIDTHS.map((w, i) => (w ? <SkText key={i} as="div" w={w} /> : <br key={i} />))}
      </div>
    </SkeletonProvider>
  );
}
