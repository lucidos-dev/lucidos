import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const inputCss = readFileSync(resolve(here, '../../../styles/chat/input-messages.css'), 'utf-8');
const responseCss = readFileSync(resolve(here, '../../../styles/chat/response.css'), 'utf-8');

function getBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
  return [...css.matchAll(re)].map(m => m[1]).join('\n');
}

function declarationValue(block: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return block.match(new RegExp(`${escaped}\\s*:\\s*([^;]+)`))?.[1].trim();
}

describe('turn header gutter', () => {
  it('keeps actor and executor icons aligned with turn body content', () => {
    const initiatorHeader = getBlock(inputCss, '.initiator-header');
    const initiatorBody = getBlock(inputCss, '.initiator-body');
    const responseHeader = getBlock(responseCss, '.response-header');
    const responseContent = getBlock(responseCss, '.response-content');

    expect(declarationValue(initiatorHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(initiatorBody, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseContent, 'padding-left')).toBe('var(--turn-body-inset)');
  });

  it('insets the collapsed marker to match the turn body content', () => {
    const turnCollapsed = getBlock(inputCss, '.turn-collapsed');
    expect(declarationValue(turnCollapsed, 'padding-left')).toBe('var(--turn-body-inset)');
  });

  // The panels carry the RIGHT inset in the BASE layout (mirroring the content's
  // left inset above), so a turn sits symmetrically inside the pane. This is
  // load-bearing for the nav focus marker: it lets both marker rules below keep
  // their horizontal sides at 0/base — no rightward box growth — so the marker's
  // border gets equal breathing room from both pane edges instead of sitting
  // flush against the right one ("border is all the way to the right").
  it('gives the panels a base right inset matching the content left inset', () => {
    const base = inputCss.match(/\.initiator-panel\s*,\s*\.response-panel\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(base).not.toBe('');
    expect(declarationValue(base, 'padding')).toBe(
      'var(--turn-feed-pad-y) var(--turn-body-inset) var(--turn-feed-pad-y) 0',
    );
  });

  // The feed rhythm separates a turn from the NEXT one, so the LAST turn has
  // nothing below it for its bottom half to separate from: there it stacked on
  // the transcript's own bottom padding (--prompt-fade + --nav-focus-reach) and
  // opened a hole under the running step, floating the reply's live edge clear
  // of the composer. Both panel kinds are covered, because a turn whose
  // response has not started ends on its initiator panel.
  it('drops the feed rhythm below the last turn, where nothing follows it', () => {
    const re = /\.thread-content > \.chat-exchange:last-child > ([^{]*)\{([^}]*)\}/;
    const [, selectorTail, block] = inputCss.match(re) ?? [];
    expect(block, 'no last-turn rule').toBeTruthy();
    expect(declarationValue(block!, 'padding-bottom')).toBe('0');

    for (const panel of ['initiator-panel', 'response-panel']) {
      // Both panel kinds, because either can be the turn's last child: a turn
      // whose response has not started yet ends on its initiator panel.
      expect(selectorTail, `.${panel} not covered`).toContain(`.${panel}:last-child`);
    }
    // It must YIELD to the panel-level nav focus marker. At (0,5,0) it
    // out-ranks that rule's (0,2,0), so without the exclusion a deep link
    // landing on the last turn paints the marker's wash flush against the last
    // line while the other three sides keep their inset, which is the exact
    // asymmetry the marker rule exists to remove.
    expect(selectorTail).toContain(':not(.nav-focus-stuck)');
  });

  // The action footer (Diff/Revert on a change card) is the panel's LAST child, so
  // any padding-bottom here stacks on the panel's own bottom padding and shows up as
  // an oversized BOTTOM gap under the nav focus marker — while the panel rule below
  // normalizes every side to var(--turn-body-inset). Only the TOP padding (buttons ↔
  // body) belongs on the footer; the bottom spacing is the panel's job. A shorthand
  // that re-adds a bottom value (e.g. `0.5rem 0`) regresses the change card's
  // four-side symmetry — exactly the "bottom gap is bigger than the others" report.
  it('keeps the action footer from inflating the focus marker bottom gap', () => {
    const footer = getBlock(responseCss, '.initiator-footer');
    expect(footer).not.toBe('');
    // 3-value shorthand = top / left-right / bottom, so `0.5rem 0 0` is
    // top 0.5rem, left+right 0, bottom 0 — padding-bottom is explicitly 0.
    expect(declarationValue(footer, 'padding')).toBe('0.5rem 0 0');
  });

  // The nav focus marker washes the panel box edge to edge,
  // so the gap it shows on each side equals that side's padding. The horizontal
  // sides are symmetric in the base layout (left inset on the content, right
  // inset on the panel — pinned above), so the marker rule only normalizes the
  // far larger feed padding var(--turn-feed-pad-y) TOP/BOTTOM. It must target
  // the CURRENT marker class (.nav-focus-stuck) on BOTH deep-link hosts (an
  // event / resolution card lands on .initiator-panel, a change proposing-turn
  // on .response-panel). A rename that leaves this rule on the old class names
  // silently drops it and the padding regresses (which is exactly what happened
  // when the unified focus marker landed).
  it('gives the focus-marked panels a uniform gap on all four sides', () => {
    const re =
      /\.initiator-panel\.nav-focus-stuck\s*,\s*\.response-panel\.nav-focus-stuck\s*\{([^}]*)\}/;
    const block = responseCss.match(re)?.[1] ?? '';
    expect(block).not.toBe('');
    // LEFT inset comes from the body's padding-left, RIGHT from the base panel
    // padding; the shorthand restates them (T R B L = inset inset inset 0).
    expect(declarationValue(block, 'padding')).toBe(
      'var(--turn-body-inset) var(--turn-body-inset) var(--turn-body-inset) 0',
    );
    // NO rightward box growth: a negative margin-right here pushes the marker
    // into the .thread-content gutter until it sits flush against the
    // pane's right edge while the left keeps its breathing room, the "border
    // is all the way to the right" report. The base right inset (pinned above)
    // makes the growth unnecessary.
    expect(declarationValue(block, 'margin-right')).toBeUndefined();
    // The shrunk feed padding is handed back as vertical margin, so top/bottom
    // match left/right without moving the turn's content or its neighbours.
    expect(declarationValue(block, 'margin-top')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    expect(declarationValue(block, 'margin-bottom')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    // The pre-rename class names must be gone — their presence means the rule
    // was copied, not migrated.
    expect(responseCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
  });

  // Keyboard ⌘↑/⌘↓ turn-nav marks the WHOLE TURN (.chat-exchange, TURN_SELECTOR in
  // scrollState.ts), not an inner panel — so the panel rule above never fires for it.
  // Without a counterpart rule the wash filled the exchange while the panel's feed
  // padding + left inset leaked through (top/bottom ~2× the sides, right gap collapsed
  // to nothing), the asymmetry the panel rule already fixed for deep-links.
  // The exchange rule normalizes every side to var(--turn-body-inset): a uniform
  // exchange padding, the inner left inset stripped, the first/last panel's feed
  // padding dropped, and the feed rhythm handed back as exchange margin. It can NOT
  // use the panel rule's negative margin-right (the exchange is auto-centered via
  // .thread-content > * { margin: 0 auto }; a negative margin-right would fight it).
  it('gives the focus-marked whole turn (.chat-exchange) a uniform gap too', () => {
    const block = getBlock(responseCss, '.chat-exchange.nav-focus-stuck');
    expect(block).not.toBe('');
    // Horizontal sides stay 0 — both come through the panels (LEFT from the
    // inner content's padding-left, RIGHT from the panels' base padding-right),
    // so marking the exchange never reflows its content. Only TOP/BOTTOM are
    // re-added (2-value shorthand = vertical inset, horizontal 0). No negative
    // margins either — the exchange is auto-centered
    // (.thread-content > * { margin: 0 auto }) and they would fight that.
    expect(declarationValue(block, 'padding')).toBe('var(--turn-body-inset) 0');
    expect(declarationValue(block, 'margin-right')).toBeUndefined();
    // Feed rhythm handed back as exchange margin so content / neighbours don't move.
    expect(declarationValue(block, 'margin-top')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    expect(declarationValue(block, 'margin-bottom')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    // The first/last panel's feed padding is dropped (it lives on the panel, not the
    // exchange; leaving it would stack on the exchange padding and inflate top/bottom).
    // Whitespace-tolerant regexes (selectors span lines) — mirrors the panel test.
    const firstChild =
      responseCss.match(
        /\.chat-exchange\.nav-focus-stuck > \.initiator-panel:first-child\s*,\s*\.chat-exchange\.nav-focus-stuck > \.response-panel:first-child\s*\{([^}]*)\}/,
      )?.[1] ?? '';
    expect(declarationValue(firstChild, 'padding-top')).toBe('0');
    const lastChild =
      responseCss.match(
        /\.chat-exchange\.nav-focus-stuck > \.initiator-panel:last-child\s*,\s*\.chat-exchange\.nav-focus-stuck > \.response-panel:last-child\s*\{([^}]*)\}/,
      )?.[1] ?? '';
    expect(declarationValue(lastChild, 'padding-bottom')).toBe('0');
    // Error turns append .exchange-error after the panels, so no panel is :last-child
    // and no bottom feed padding is removed — the margin-bottom above would then have
    // nothing to hand back and shove the next turn down by the exchange padding. The
    // :has() override cancels the exchange's own padding-bottom instead (net-zero).
    const errorTurn = getBlock(responseCss, '.chat-exchange.nav-focus-stuck:has(> .exchange-error:last-child)');
    expect(errorTurn).not.toBe('');
    expect(declarationValue(errorTurn, 'margin-bottom')).toBe('calc(-1 * var(--turn-body-inset))');
  });
});
