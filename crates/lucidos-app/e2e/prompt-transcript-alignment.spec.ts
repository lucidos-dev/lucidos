/**
 * The composer box (.prompt-box) lines up with the transcript's turn content on
 * BOTH edges, at every pane width and on every viewport, and so does the
 * content INSIDE it.
 *
 * Four separate things used to break that, and each is asserted here rather
 * than trusted to a comment:
 *
 *   1. `.prompt-area` carried an ASYMMETRIC gutter (0.75rem left, 1.15rem
 *      right), which pushed the centered composer half the excess off the
 *      transcript's centre line once the pane was wider than the content cap.
 *   2. The composer column spanned the full --content-max-width box while a
 *      turn insets its content by --turn-body-inset, so the composer sat that
 *      much outside the message text on both sides.
 *   3. The transcript is a scroll container and the composer is not, so on a
 *      classic-scrollbar platform the scrollbar takes its width out of the
 *      transcript's content box only. That is what --scrollbar-gutter-width
 *      (utils/scrollbarGutter.ts) hands back to the composer. It is measured off
 *      the LIVE transcript, because a detached clone of it answered 9px on real
 *      iOS where the transcript itself reserves nothing, and the composer then
 *      subtracted a gutter that was not there. No engine reproduces that split
 *      under emulation, so the guard here is the gutterVar-vs-reserved assertion
 *      below (it fails if the publish stops tracking the real element at all)
 *      plus the unit tests in utils/scrollbarGutter.test.ts.
 *   4. Matching the outer edges was not enough on its own. The composer's shell
 *      put a wider border+padding between its edge and its text than a question
 *      card puts between its edge and its text, so the typed text still sat a
 *      few px right of the card text directly above it, which is what a user
 *      reads as "the composer is misaligned on the left". Both surfaces now
 *      derive that gap from --turn-surface-inset (base.css).
 *
 * The reference edges are read as the CONTENT box of a turn's body, computed
 * from live padding rather than a hardcoded inset, so the test still measures
 * the right thing if --turn-body-inset changes.
 */
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
  /** Border+padding a question card puts between its own outer edge and its
   *  content, and the same distance for the composer box. */
  cardInset: number;
  cardInsetRight: number;
  composerInset: number;
  composerInsetRight: number;
  /** Width of the spliced-in card probe, and the gap (expected 0) between the
   *  composer box's content edge and the textarea. Both are tripwires for the
   *  measurements above rather than assertions about alignment. */
  probeWidth: number;
  rowLeftGap: number;
}

/** Content edges of the last user turn's body, the composer box's own edges,
 *  and the inner content inset of both nested surfaces. Returns null if
 *  anything isn't laid out (0-width copy on the inactive layout, transcript not
 *  rendered yet).
 *
 *  One measurement pass for every invariant in this file, deliberately: the
 *  fixture it needs is a real thread with a real turn, which costs an LLM round
 *  trip per Playwright project, and every assertion reads the same laid-out
 *  frame. */
async function measure(page: Page): Promise<Edges | null> {
  return page.evaluate(() => {
    const visible = <T extends Element>(els: NodeListOf<T>): T | null => {
      // Header chrome renders per-layout copies; take the one with real width.
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

    // A `.question-option` probe is spliced into the live transcript rather than
    // waiting for a real pending question, so the card's inset is read off the
    // same cascade a real card would get. Removed before anything can paint.
    const probe = document.createElement('button');
    probe.className = 'question-option';
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
      // LEFT is measured geometrically: the textarea is `.prompt-row`'s first
      // child and the row has no padding, so its content edge IS the
      // composer's, and this catches anything creeping in between.
      composerInset: taRect.left + parseFloat(taStyle.paddingLeft) - boxRect.left,
      // RIGHT is summed from styles instead, because the textarea does NOT reach
      // the box's right content edge: `.prompt-clear` is an in-flow flex sibling
      // after it, and it only goes `visibility: hidden` when the field is empty,
      // so it keeps its width, margin and the row gap either way. Measuring
      // geometrically here would fold that button in and fail on a composer that
      // is correctly aligned.
      composerInsetRight:
        parseFloat(boxStyle.borderRightWidth) + parseFloat(taStyle.paddingRight),
      // Tripwire for the assumption the LEFT measurement rests on.
      rowLeftGap: taRect.left - (boxRect.left + parseFloat(boxStyle.borderLeftWidth)),
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

    // The published gutter must equal what the transcript's scroll container
    // actually reserved, or the composer compensates by the wrong amount. The
    // publish reads that element directly, so this catches the reverse: the
    // re-publish never running (ThreadView's mount effect), leaving the boot
    // probe's estimate live on a platform where it does not match.
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

    // Matching the outer edges above is not enough: the composer docks under a
    // stack of question / change cards, and the eye reads the text, not the box.
    // So the border+padding each nested surface puts between its own edge and
    // its content has to match too.
    //
    // Tripwires for the two measurements below, before their comparison runs.
    expect(e.probeWidth, 'question-option probe did not lay out').toBeGreaterThan(0);
    expect(
      e.rowLeftGap,
      `something sits between .prompt-box's content edge and the textarea (${e.rowLeftGap}px)`,
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
