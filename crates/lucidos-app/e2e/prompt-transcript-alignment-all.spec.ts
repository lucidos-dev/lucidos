import { test, expect, Page } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
} from './helpers';

interface Edges {
  turnLeft: number;
  turnRight: number;
  boxLeft: number;
  boxRight: number;
  gutterVar: string;
  reservedByScrollbar: number;
  cardInset: number;
  cardInsetRight: number;
  composerInset: number;
  composerInsetRight: number;
  probeWidth: number;
  rowLeftGap: number;
  rowRightGap: number;
}

async function measure(page: Page): Promise<Edges | null> {
  return page.evaluate(() => {
    const visible = <T extends Element>(els: NodeListOf<T>): T | null => {
      for (let i = els.length - 1; i >= 0; i--) {
        if (els[i].getBoundingClientRect().width > 0) return els[i];
      }
      return null;
    };
    const body = visible(
      document.querySelectorAll<HTMLElement>('.initiator-panel-user .initiator-body'),
    );
    const box = visible(document.querySelectorAll<HTMLElement>('.prompt-box'));
    const scroller = visible(document.querySelectorAll<HTMLElement>('.thread-content'));
    const textarea = visible(
      document.querySelectorAll<HTMLElement>('.prompt-row .prompt-textarea'),
    );
    if (!body || !box || !scroller || !textarea) return null;
    const bodyRect = body.getBoundingClientRect();
    const bodyStyle = getComputedStyle(body);
    const boxRect = box.getBoundingClientRect();
    const boxStyle = getComputedStyle(box);

    const probe = document.createElement('div');
    probe.className = 'question-body protected-surface';
    scroller.appendChild(probe);
    const probeWidth = probe.getBoundingClientRect().width;
    const probeStyle = getComputedStyle(probe);
    const cardInset =
      parseFloat(probeStyle.borderLeftWidth) + parseFloat(probeStyle.paddingLeft);
    const cardInsetRight =
      parseFloat(probeStyle.borderRightWidth) + parseFloat(probeStyle.paddingRight);
    probe.remove();

    const taRect = textarea.getBoundingClientRect();
    const taStyle = getComputedStyle(textarea);
    return {
      turnLeft: bodyRect.left + parseFloat(bodyStyle.paddingLeft),
      turnRight: bodyRect.right - parseFloat(bodyStyle.paddingRight),
      boxLeft: boxRect.left,
      boxRight: boxRect.right,
      gutterVar: getComputedStyle(document.documentElement)
        .getPropertyValue('--scrollbar-gutter-width')
        .trim(),
      reservedByScrollbar: scroller.offsetWidth - scroller.clientWidth,
      cardInset,
      cardInsetRight,
      probeWidth,
      composerInset: taRect.left + parseFloat(taStyle.paddingLeft) - boxRect.left,
      composerInsetRight: boxRect.right - (taRect.right - parseFloat(taStyle.paddingRight)),
      rowLeftGap: taRect.left - (boxRect.left + parseFloat(boxStyle.borderLeftWidth)),
      rowRightGap: (boxRect.right - parseFloat(boxStyle.borderRightWidth)) - taRect.right,
    };
  });
}

test.describe('Composer aligns with the transcript content', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('composer box and its content match the turn content edges', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('align');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);
    await waitForResponse(page);

    const edges = await measure(page);
    expect(edges, 'user turn body / prompt box not laid out').not.toBeNull();
    const e = edges!;

    expect(
      parseFloat(e.gutterVar),
      `--scrollbar-gutter-width=${e.gutterVar} but .thread-content reserved ${e.reservedByScrollbar}px`,
    ).toBeCloseTo(e.reservedByScrollbar, 0);

    expect(
      e.boxLeft,
      `composer left=${e.boxLeft} vs turn content left=${e.turnLeft}`,
    ).toBeCloseTo(e.turnLeft, 0);
    expect(
      e.boxRight,
      `composer right=${e.boxRight} vs turn content right=${e.turnRight}`,
    ).toBeCloseTo(e.turnRight, 0);

    expect(e.probeWidth, 'question-body probe did not lay out').toBeGreaterThan(0);
    expect(
      e.rowLeftGap,
      `something sits between .prompt-box's content edge and the textarea (${e.rowLeftGap}px)`,
    ).toBeCloseTo(0, 0);
    expect(
      e.rowRightGap,
      `something sits between the textarea and .prompt-box's right content edge (${e.rowRightGap}px)`,
    ).toBeCloseTo(0, 0);

    expect(
      e.composerInset,
      `composer content inset=${e.composerInset} vs card content inset=${e.cardInset}`,
    ).toBeCloseTo(e.cardInset, 0);
    expect(
      e.composerInsetRight,
      `composer right inset=${e.composerInsetRight} vs card right inset=${e.cardInsetRight}`,
    ).toBeCloseTo(e.cardInsetRight, 0);
  });
});
