import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport, disarmFollowSeed } from './helpers';

/** Scrolling away from a reply IN FLIGHT retires the standing follow.
 *
 *  Its sibling `thread-scroll-belongs-to-the-reader.spec.ts` walks the toggle's
 *  whole journey, and says in its own header that every grow it makes lands on
 *  an IDLE thread: `waitForResponse` has returned by then. So the one state the
 *  disarm exists for, a reader fleeing a reply the agent is still writing, had
 *  no browser coverage at all. This file is that state and nothing else.
 *
 *  It has to be a BROWSER test rather than another case in
 *  `scroll-follow-the-live-edge.test.ts`. That suite runs DOM-free (the project
 *  has no jsdom) against a hand-built container, so it can only assert what the
 *  module DECIDES once told a gesture happened; `reader-gesture-listeners.test.ts`
 *  covers which inputs produce one, but through a fake listener registry rather
 *  than a browser's. Neither can see the two things only a real engine has:
 *  whether a real wheel over a real transcript reaches the listener at all, and
 *  whether the live term is actually TRUE while the agent works
 *  (`setThreadLive`, published from `ChatExchange` off the thread projection,
 *  which no unit test drives).
 *
 *  Desktop only, like its sibling: the mobile header's own scroll compensation
 *  is a second writer over the same offset. */

/** How far the reader's wheel takes them off the live edge. Comfortably past
 *  `isAtLiveEdge`'s 2px slack, and small enough to fit in the overflow a single
 *  turn produces. */
const WHEEL_TRAVEL_PX = 200;

/** A user message tall enough that the transcript overflows on its own.
 *
 *  The height has to come from the PROMPT rather than from the reply, because
 *  the e2e provider is `MockProvider`: it answers every prompt with the same
 *  ~100-word paragraph, so a request for 250 numbers produces the same 220px of
 *  overflow as any other and no wording makes the reply taller. Waiting for the
 *  transcript to grow past that is what the first two attempts at this spec did,
 *  and both spent 60s polling a number that never moved. What the mock DOES give
 *  is a real streaming window: it emits word by word with a 30ms delay, so the
 *  turn is live for roughly three seconds, which is the window this spec acts
 *  in. */
const TALL_MESSAGE = Array.from({ length: 30 }, (_, i) => `Context line ${i + 1}.`).join('\n');

test.describe('Scrolling away from a live reply retires the follow (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a wheel up while the agent is still writing turns the toggle off', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    // Short viewport so a modest transcript overflows: with no scroll capacity
    // there is nowhere to scroll away TO and the assertion is vacuous.
    await page.setViewportSize({ width: 1280, height: 400 });
    // The ride has to be OFF here, so the click below is the arm this spec is
    // about. The seed ships armed, and a fresh context has no press to say
    // otherwise.
    await disarmFollowSeed(page);
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const toggle = page.locator('button[data-role="follow-live-edge"]:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
    const overflow = () => tc.evaluate(el => el.scrollHeight - el.clientHeight);

    /** Is a turn in flight, by the same reading the panel shows the user? The
     *  closest a spec can get to the live term itself: both come from the last
     *  exchange's status, and this one is what the reader sees when they decide
     *  the thread is "live". */
    const agentIsWorking = () => page.evaluate(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      for (const label of labels) {
        const rect = label.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) continue;
        const text = label.textContent ?? '';
        if (text.includes('Working') || text.includes('Requesting')) return true;
      }
      return false;
    });

    // One settled turn first, so the transcript already overflows before the
    // turn under test starts. Nothing about the reply is relied on here beyond
    // its arrival.
    await sendMessage(page, TALL_MESSAGE);
    await waitForResponse(page);
    expect(await overflow()).toBeGreaterThan(WHEEL_TRAVEL_PX + 20);

    // Arm the ride. On a settled thread the toggle still takes the reader to the
    // live edge, which is where the turn under test has to start from.
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(atBottom).toBe(true);

    // THE TURN UNDER TEST. Not awaited: the whole point is to act while it runs.
    await sendMessage(page, TALL_MESSAGE);
    await expect.poll(agentIsWorking, { timeout: 30_000 }).toBe(true);
    // The submit's own glide has to land before the wheel, or the gesture would
    // be cancelling a tween rather than retiring a settled ride. Riding readers
    // are taken to the live edge by a submit, so that is where it lands.
    await expect.poll(atBottom).toBe(true);

    expect(await agentIsWorking(), 'the reply finished before the wheel could land').toBe(true);

    // THE GESTURE. A real wheel over the transcript, which is the input the
    // whole disarm hangs off: the position test alone cannot tell the reader
    // from the platform, so `scrollState` asks whether a gesture was behind the
    // scroll (see "Was this scroll the reader's own GESTURE?").
    const box = await tc.boundingBox();
    if (!box) throw new Error('transcript has no box');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -WHEEL_TRAVEL_PX);

    // The toggle goes off BY ITSELF. It renders the follow rather than owning
    // it, so this is the module's disarm surfacing, not a second rule.
    await expect(toggle).toHaveAttribute('aria-pressed', 'false', { timeout: 5_000 });

    // And the ride really stopped: the rest of the reply arrives without
    // dragging the reader back down.
    await expect.poll(atBottom).toBe(false);
    const parked = await tc.evaluate(el => el.scrollTop);
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(parked);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);
  });

  /* THE SCROLLBAR is the other way a desktop reader moves a transcript, and it
   * is deliberately NOT covered here: Playwright cannot drive Chromium's
   * scrollbar. A drag was written and run, and its own harness check (drag the
   * thumb with nothing riding, assert the container moved) failed with
   * scrollTop unchanged at 766, so the synthetic pointer events never grip the
   * thumb. Chromium hit-tests its scrollbars outside the path CDP's
   * `Input.dispatchMouseEvent` feeds, so there is nothing to aim at.
   *
   * That leaves the gutter press covered only by
   * `src/components/chat/__tests__/reader-gesture-listeners.test.ts`, which
   * builds the `pointerdown` itself and therefore assumes the two things a
   * browser would have to prove: that Chromium dispatches one to the element
   * for a scrollbar press at all, and that `offsetX` really does exceed
   * `clientWidth` there. Worth knowing when reading that test, and worth not
   * re-attempting here without first checking whether the driver has grown the
   * ability. */
});
