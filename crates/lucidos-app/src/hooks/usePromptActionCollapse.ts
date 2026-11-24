import { useState, useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow, contentWidthOf } from './useFitsInOneRow';

export const FOLD_KEY_ATTR = 'data-fold-key';

const GAPPED_CLUSTER = '.prompt-actions-right';

export type FoldGroup = 'row' | 'cluster';

export interface PromptCollapseInput {
  containerWidth: number;
  pinnedWidth: number;
  memberWidths: readonly number[];
  memberGroups: readonly FoldGroup[];
  pinnedClusterCount: number;
  gapPx: number;
  moreWidth: number;
  merge?: { first: number; count: number; width: number; atFold: number } | null;
}

export interface PromptCollapse {
  folded: number;
  merged: boolean;
}

const STANDING: PromptCollapse = { folded: 0, merged: false };

export function computePromptCollapse(input: PromptCollapseInput): PromptCollapse {
  const {
    containerWidth, pinnedWidth, memberWidths, memberGroups,
    pinnedClusterCount, gapPx, moreWidth, merge = null,
  } = input;
  const total = memberWidths.length;
  const groupEnd = merge ? merge.first + merge.count : 0;
  const candidates: PromptCollapse[] = [];
  const unmergedTo = merge ? Math.min(merge.atFold, total) : total;
  for (let folded = 0; folded <= unmergedTo; folded++) candidates.push({ folded, merged: false });
  if (merge) {
    for (let folded = unmergedTo; folded <= total; folded++) {
      const insideGroup = folded > merge.first && folded < groupEnd;
      if (!insideGroup) candidates.push({ folded, merged: true });
    }
  }

  let best = candidates[0];
  let bestWidth = Infinity;
  for (const candidate of candidates) {
    let standing = 0;
    let inCluster = pinnedClusterCount;
    for (let i = candidate.folded; i < total; i++) {
      if (merge && candidate.merged && i === merge.first) {
        standing += merge.width;
        if (memberGroups[i] === 'cluster') inCluster++;
        i = groupEnd - 1;
        continue;
      }
      standing += memberWidths[i];
      if (memberGroups[i] === 'cluster') inCluster++;
    }
    const width = (candidate.folded > 0 ? moreWidth : 0) + standing + Math.max(0, inCluster - 1) * gapPx;
    if (computeFitsInOneRow([pinnedWidth, width], containerWidth, 0, 0)) return candidate;
    if (width < bestWidth) { bestWidth = width; best = candidate; }
  }
  return best;
}

export interface PromptMerge {
  keys: readonly string[];
  foldKey: string;
  atKey: string;
}

export function usePromptActionCollapse(
  containerRef: RefObject<HTMLElement>,
  memberKeys: readonly string[],
  memberGroups: readonly FoldGroup[],
  merge: PromptMerge | null = null,
  compact = false,
  gapRem = 0.5,
): PromptCollapse {
  const [collapse, setCollapse] = useState<PromptCollapse>(STANDING);
  const widths = useRef<Map<string, number>>(new Map());
  const keys = memberKeys.join(' ');
  const mergeKeys = merge?.keys.join(' ') ?? null;
  const mergeFoldKey = merge?.foldKey ?? null;
  const mergeAtKey = merge?.atKey ?? null;
  useLayoutEffect(() => {
    widths.current = new Map();
    setCollapse(STANDING);
  }, [keys, compact]);
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => {
      const seen = widths.current;
      let pinnedWidth = 0;
      let moreWidth = 0;
      let pinnedClusterCount = 0;
      container.querySelectorAll<HTMLElement>('[data-row-item]').forEach((item) => {
        const width = item.getBoundingClientRect().width;
        const key = item.getAttribute(FOLD_KEY_ATTR);
        if (key === '') { moreWidth = width; return; }
        if (key !== null) { seen.set(key, width); return; }
        pinnedWidth += width;
        if (item.closest(GAPPED_CLUSTER)) pinnedClusterCount++;
      });
      const fallback = moreWidth || seen.values().next().value || 0;
      const mergeFirst = merge ? memberKeys.indexOf(merge.keys[0]) : -1;
      const mergeAt = merge ? memberKeys.indexOf(merge.atKey) : -1;
      const mergeSpec = merge && mergeFirst !== -1 && mergeAt !== -1 ? {
        first: mergeFirst,
        count: merge.keys.length,
        width: seen.get(merge.foldKey)
          ?? Math.min(...merge.keys.map((k) => seen.get(k) ?? fallback)),
        atFold: mergeAt,
      } : null;
      const next = computePromptCollapse({
        containerWidth: contentWidthOf(container),
        pinnedWidth,
        memberWidths: memberKeys.map((k) => seen.get(k) ?? fallback),
        memberGroups,
        pinnedClusterCount,
        gapPx: gapRem * getRemPx(),
        moreWidth: moreWidth || fallback,
        merge: mergeSpec,
      });
      setCollapse((prev) => (
        prev.folded === next.folded && prev.merged === next.merged ? prev : next
      ));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    const mo = new MutationObserver(measure);
    mo.observe(container, { childList: true, subtree: true, characterData: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, [containerRef, keys, compact, gapRem, memberKeys, memberGroups, mergeKeys, mergeFoldKey, mergeAtKey]);
  return collapse;
}
