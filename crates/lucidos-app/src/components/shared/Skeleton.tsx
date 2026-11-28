import { createContext, cloneElement, type ComponentChildren, type VNode } from 'preact';
import { useContext, useLayoutEffect, useRef, useState } from 'preact/hooks';

const FILL_FALLBACK_ROWS = 8;

const SkeletonContext = createContext(false);

export function useSkeleton(): boolean {
  return useContext(SkeletonContext);
}

export function SkeletonProvider({ children }: { children: ComponentChildren }) {
  return <SkeletonContext.Provider value={true}>{children}</SkeletonContext.Provider>;
}

function SkBar({
  w,
  h,
  circle,
  round,
}: {
  w?: string;
  h?: string;
  circle?: boolean;
  round?: boolean;
}) {
  return (
    <span
      class={`sk-bar${circle ? ' sk-circle' : ''}${round ? ' sk-round' : ''}`}
      style={{ width: w, height: h }}
      aria-hidden="true"
    />
  );
}

export function SkText({
  class: cls,
  w,
  as = 'span',
  children,
}: {
  class?: string;
  w?: string;
  as?: 'span' | 'div';
  children?: ComponentChildren;
}) {
  const Tag = as;
  if (useSkeleton()) {
    return (
      <Tag class={cls} aria-hidden="true">
        <SkBar w={w ?? '60%'} />
      </Tag>
    );
  }
  return <Tag class={cls}>{children}</Tag>;
}

export function SkBlock({
  w,
  h,
  circle,
  round,
  children,
}: {
  w?: string;
  h?: string;
  circle?: boolean;
  round?: boolean;
  children?: ComponentChildren;
}) {
  if (useSkeleton()) return <SkBar w={w} h={h} circle={circle} round={round} />;
  return <>{children}</>;
}

export function computeFillCount(availablePx: number, stridePx: number): number {
  if (availablePx <= 0 || stridePx <= 0) return FILL_FALLBACK_ROWS;
  return Math.ceil(availablePx / stridePx) + 1;
}

export function ListSkeletonOf({
  row,
  count = 5,
  fill = false,
  containerClass,
}: {
  row: (i: number) => VNode;
  count?: number;
  fill?: boolean;
  containerClass?: string;
}) {
  if (fill) return <FillSkeleton row={row} containerClass={containerClass} />;
  return (
    <SkeletonProvider>
      <div class={containerClass} aria-hidden="true">
        {Array.from({ length: count }, (_, i) => cloneElement(row(i), { key: i }))}
      </div>
    </SkeletonProvider>
  );
}

function paneEndSpacePx(pane: Element, el: Element): number {
  const view = Array.from(pane.children).find((child) => child.contains(el));
  return view ? parseFloat(getComputedStyle(view, '::after').height) || 0 : 0;
}

function FillSkeleton({ row, containerClass }: { row: (i: number) => VNode; containerClass?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [heightPx, setHeightPx] = useState<number | null>(null);
  const [count, setCount] = useState(FILL_FALLBACK_ROWS);
  useLayoutEffect(() => {
    const el = ref.current;
    const pane = el?.closest('.content-pane-body');
    if (!el || !pane) return;
    const measure = () => {
      const available = pane.getBoundingClientRect().bottom - el.getBoundingClientRect().top - paneEndSpacePx(pane, el);
      if (available <= 0) return;
      const rendered = el.children.length;
      const stride = rendered > 0 ? el.scrollHeight / rendered : 0;
      setHeightPx(available);
      setCount(computeFillCount(available, stride));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(pane);
    return () => ro.disconnect();
  }, []);
  return (
    <SkeletonProvider>
      <div
        ref={ref}
        class={`${containerClass ?? ''} sk-fill`.trim()}
        style={heightPx == null ? undefined : { height: `${heightPx}px` }}
        aria-hidden="true"
      >
        {Array.from({ length: count }, (_, i) => cloneElement(row(i), { key: i }))}
      </div>
    </SkeletonProvider>
  );
}
