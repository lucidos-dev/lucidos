import { test, expect } from './fixtures';
import { overflowsPane } from './transcript-fit';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

test.describe('The transcript ends where its content ends', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('reserves no air, holds a short thread still, and still shows what was sent', async ({ page }) => {
    if (!isMobileViewport(page)) await page.setViewportSize({ width: 1280, height: 600 });
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();

    const measure = () => tc.evaluate((el) => {
      const feed = el.querySelector('.thread-feed') as HTMLElement | null;
      const turns = feed ? feed.querySelectorAll('.chat-exchange') : [];
      const last = turns[turns.length - 1] as HTMLElement | undefined;
      if (!feed || !last) return null;
      if (last.getClientRects().length === 0 || getComputedStyle(last).minHeight === '') return null;
      const top = el.getBoundingClientRect().top;
      const header = last.querySelector('.response-header') as HTMLElement | null;
      const initiator = last.querySelector('.initiator-panel') as HTMLElement | null;
      const tail = feed.lastElementChild as HTMLElement;
      const tailBottom = tail.getBoundingClientRect().bottom - top + el.scrollTop;
      const head = feed.firstElementChild as HTMLElement;
      return {
        trailing: el.scrollHeight - tailBottom,
        paddingBottom: parseFloat(getComputedStyle(el).paddingBottom) || 0,
        tailClass: tail.className,
        headInset: head.getBoundingClientRect().top - feed.getBoundingClientRect().top,
        minHeight: getComputedStyle(last).minHeight,
        turnHeight: last.getBoundingClientRect().height,
        messageTop: initiator ? initiator.getBoundingClientRect().top - top : null,
        messageBottom: initiator ? initiator.getBoundingClientRect().bottom - top : null,
        statusLineBottom: header ? header.getBoundingClientRect().bottom - top : null,
        clientHeight: el.clientHeight,
        scrollHeight: el.scrollHeight,
        titleTop: (() => {
          const row = el.querySelector('.mobile-thread-title-row') as HTMLElement | null;
          if (!row || getComputedStyle(row).display === 'none') return null;
          return row.getBoundingClientRect().top - top;
        })(),
        spacerHeight: parseFloat(getComputedStyle(el, '::before').height) || 0,
      };
    });

    const geometry = async () => {
      const m = await measure();
      return m && { ...m, scrollable: overflowsPane(m.scrollHeight, m.clientHeight) };
    };

    let sawUnscrollable = false;

    const expectNoAirAndNothingMoved = async (when: string) => {
      let g!: NonNullable<Awaited<ReturnType<typeof geometry>>>;
      await expect.poll(async () => {
        const measured = await geometry();
        if (measured) g = measured;
        return measured !== null;
      }, { message: `${when}: no turn rendered inside a feed to measure` }).toBe(true);
      if (!g.scrollable) sawUnscrollable = true;
      if (g.scrollable) {
        expect(
          Math.abs(g.trailing - g.paddingBottom),
          `${when}: ${Math.round(g.trailing)}px under the last child (.${g.tailClass}), against ${Math.round(g.paddingBottom)}px of padding`,
        ).toBeLessThanOrEqual(1);
      } else {
        expect(
          g.headInset,
          `${when}: the feed's content starts ${Math.round(g.headInset)}px below the feed's own top`,
        ).toBeLessThanOrEqual(1);
      }
      expect(['0px', 'auto'], `${when}: the last turn is floored`).toContain(g.minHeight);
      if (g.titleTop !== null) {
        expect(
          g.titleTop,
          `${when}: the sticky title sits ${Math.round(g.titleTop)}px down the pane, against a ${Math.round(g.spacerHeight)}px spacer`,
        ).toBeLessThanOrEqual(g.spacerHeight + 1);
      }
    };

    await sendMessage(page, 'Reply with exactly the word: one');
    await expectNoAirAndNothingMoved('the moment the first message is sent');
    await waitForResponse(page);
    await expectNoAirAndNothingMoved('the first reply is in');

    await sendMessage(page, 'Reply with exactly the word: two');
    await expectNoAirAndNothingMoved('the moment the second message is sent');
    await waitForResponse(page);
    await expectNoAirAndNothingMoved('the second reply is in');

    let landed!: NonNullable<Awaited<ReturnType<typeof geometry>>>;
    await expect.poll(async () => {
      const measured = await geometry();
      if (measured) landed = measured;
      return measured !== null;
    }, { message: 'no turn rendered inside a feed to measure once both replies are in' }).toBe(true);
    expect(landed.scrollable, 'the thread never became scrollable').toBe(true);

    expect(sawUnscrollable, 'no round measured a transcript short of the pane').toBe(true);

    const turnOverflow = Math.max(
      0, landed.turnHeight + landed.paddingBottom - landed.clientHeight);
    expect(landed.messageTop).not.toBeNull();
    expect(
      landed.messageTop!,
      `the sent message is ${Math.round(-landed.messageTop!)}px above the viewport, `
        + `against a turn that overflows the pane by ${Math.round(turnOverflow)}px`,
    ).toBeGreaterThanOrEqual(-turnOverflow - 1);
    expect(landed.messageBottom!, 'the sent message is below the fold').toBeLessThanOrEqual(landed.clientHeight);
    expect(landed.statusLineBottom).not.toBeNull();
    expect(landed.statusLineBottom!, 'the status line is below the fold').toBeLessThanOrEqual(landed.clientHeight);

    await tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const grown = document.createElement('div');
      grown.style.height = '2000px';
      (turns[turns.length - 1] as HTMLElement).appendChild(grown);
    });
    await expect.poll(async () => {
      const g = await geometry();
      return g ? g.trailing - g.paddingBottom : Number.POSITIVE_INFINITY;
    }, { message: 'growing the turn left air under it' }).toBeLessThanOrEqual(1);
  });
});
