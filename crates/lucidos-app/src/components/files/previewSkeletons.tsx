import { LineNumberedCode, type WideLineMode } from './LineNumberedCode';
import { SkText, SkeletonProvider } from '../shared/Skeleton';

export function FileSourceSkeleton({ wideLines }: { wideLines: WideLineMode }) {
  return (
    <SkeletonProvider>
      <LineNumberedCode rows={[]} wideLines={wideLines} />
    </SkeletonProvider>
  );
}

const PROSE_LINE_WIDTHS = ['38%', '94%', '88%', '72%', '', '91%', '84%', '56%'];

export function ProseSkeleton({ class: cls = 'response-content markdown-content' }: { class?: string }) {
  return (
    <SkeletonProvider>
      <div class={cls} aria-hidden="true">
        {PROSE_LINE_WIDTHS.map((w, i) => (w ? <SkText key={i} as="div" w={w} /> : <br key={i} />))}
      </div>
    </SkeletonProvider>
  );
}
