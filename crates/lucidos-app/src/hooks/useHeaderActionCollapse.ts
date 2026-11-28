import { useState, useLayoutEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow } from './useFitsInOneRow';

export interface HeaderCollapseInput {
  containerWidth: number;
  leadingWidth: number;
  centreWidth: number;
  actionWidths: readonly number[];
  anchorWidth: number;
  moreWidth: number;
  gapPx: number;
}

export interface HeaderCollapseResult {
  collapsed: number;
  titleEllipsized: boolean;
}

export function iconsRowWidth(
  input: Pick<HeaderCollapseInput, 'actionWidths' | 'anchorWidth' | 'moreWidth' | 'gapPx'>,
  collapsed: number,
): number {
  const widths: number[] = [];
  if (collapsed > 0) widths.push(input.moreWidth);
  for (let i = collapsed; i < input.actionWidths.length; i++) widths.push(input.actionWidths[i]);
  if (input.anchorWidth > 0) widths.push(input.anchorWidth);
  if (widths.length === 0) return 0;
  let total = 0;
  for (const w of widths) total += w;
  return total + (widths.length - 1) * input.gapPx;
}

export function computeHeaderCollapse(input: HeaderCollapseInput): HeaderCollapseResult {
  const n = input.actionWidths.length;
  const candidates: number[] = [0];
  for (let c = 2; c <= n; c++) candidates.push(c);

  const fits = (c: number): boolean => computeFitsInOneRow(
    [input.leadingWidth, input.centreWidth, iconsRowWidth(input, c)],
    input.containerWidth,
    input.gapPx,
  );

  for (const c of candidates) {
    if (fits(c)) return { collapsed: c, titleEllipsized: false };
  }
  return { collapsed: candidates[candidates.length - 1], titleEllipsized: true };
}

export interface MeasuredHeaderRow {
  clientWidth: number;
  trailingPadding: number;
  titleWidth: number;
  iconWidth: number;
  actionCount: number;
  anchorWidth: number;
  gapPx: number;
}

export function measuredCollapseInput(row: MeasuredHeaderRow): HeaderCollapseInput {
  return {
    containerWidth: row.clientWidth - row.trailingPadding,
    leadingWidth: Math.max(0, (row.clientWidth - row.titleWidth) / 2),
    centreWidth: row.titleWidth,
    actionWidths: Array.from({ length: row.actionCount }, () => row.iconWidth),
    anchorWidth: row.anchorWidth,
    moreWidth: row.iconWidth,
    gapPx: row.gapPx,
  };
}

export function mobileCollapseCount(actionCount: number): number {
  return actionCount === 1 ? 0 : actionCount;
}

export interface HeaderCollapseTargets {
  container: string;
  centre: string;
  anchor?: string;
}

export function useHeaderActionCollapse(
  hostRef: RefObject<HTMLElement>,
  actionCount: number,
  layout: 'desktop' | 'mobile',
  targets: HeaderCollapseTargets,
  opts: {
    gapRem?: number;
  } = {},
): number {
  const { gapRem = 0.25 } = opts;
  const [collapsed, setCollapsed] = useState(0);
  useLayoutEffect(() => {
    if (layout === 'mobile') return;

    const host = hostRef.current;
    if (!host) { setCollapsed(0); return; }

    const container = host.closest<HTMLElement>(targets.container);
    if (!container) {
      setCollapsed(0);
      return;
    }
    const titleZone = container.querySelector<HTMLElement>(targets.centre);
    const measure = () => {
      const stick = host.querySelector<HTMLElement>('.icon-btn');
      const iconWidth = stick ? stick.getBoundingClientRect().width : 0;
      const anchor = targets.anchor ? host.querySelector<HTMLElement>(targets.anchor) : null;
      const titleWidth = titleZone ? titleZone.getBoundingClientRect().width : 0;
      const { collapsed: next } = computeHeaderCollapse(measuredCollapseInput({
        clientWidth: container.clientWidth,
        trailingPadding: parseFloat(getComputedStyle(container).paddingRight) || 0,
        titleWidth,
        iconWidth,
        actionCount,
        anchorWidth: anchor ? anchor.getBoundingClientRect().width : 0,
        gapPx: gapRem * getRemPx(),
      }));
      setCollapsed(next);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    if (titleZone) ro.observe(titleZone);
    const hostMo = new MutationObserver(measure);
    hostMo.observe(host, { childList: true });
    return () => {
      ro.disconnect();
      hostMo.disconnect();
    };
  }, [hostRef, actionCount, gapRem, layout, targets.container,
      targets.centre, targets.anchor]);
  if (layout === 'mobile') return mobileCollapseCount(actionCount);
  return Math.min(collapsed, actionCount);
}
