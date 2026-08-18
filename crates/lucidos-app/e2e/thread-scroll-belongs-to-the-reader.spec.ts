import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport, disarmFollowSeed } from './helpers';

/** The transcript's scroll position belongs to the reader: the app moves it only
 *  when the reader asks, and exactly ONE of those asks is STANDING rather than
 *  one-shot. The FOLLOW TOGGLE in the prompt row means "take me to the live edge
 *  and keep me there", and rides until the reader scrolls away. Nothing else
 *  arms it: not the down chevron, which navigates and no more, and not a SUBMIT,
 *  which gets a one-shot landing and is over when it is done.
 *
 *  What the transcript reserves under its newest turn (nothing) has its own spec
 *  (`transcript-ends-where-its-content-ends-desktop.spec.ts`), because that is
 *  real layout and no fake container can answer it. What this one uses a submit
 *  for is the contrast: after a send the reader is left exactly where the landing
 *  put them, and the reply then grows past the fold without taking them with it.
 *
 *  It opens on a BRAND-NEW thread, whose first message is the whole of what is on
 *  screen, so there is nowhere to land and the send moves nobody at all.
 *
 *  This file was `thread-open-lands-at-end-desktop.spec.ts` and asserted a
 *  time-boxed pin: a reply landed the reader on the newest turn, and a late grow
 *  KEPT them there for 500ms. The intermittent report behind it ("scrolling to
 *  end when opening thread sometimes doesnt work") came from a growth arriving
 *  after that window, which the ResizeObserver then read as the reader having
 *  scrolled up. Both the pin and the inference are gone. What replaced them is
 *  the flag this spec walks: armed by the reader, honoured for as long as they
 *  leave it armed, and retired the moment they scroll.
 *
 *  Growing the last turn from the page (rather than waiting for a markdown image
 *  to decode late) reproduces the mechanism deterministically: a resize with NO
 *  accompanying render, which is exactly the case no layout effect covers, and
 *  where both a stale re-pin and a follow that failed to follow would show.
 *
 *  Every grow here lands on an IDLE thread, which is the state a spec is in once
 *  `waitForResponse` returns, and an idle thread carries nobody: growth there is
 *  the transcript finishing its own rendering. So the follow's CARRYING is
 *  asserted the only deterministic way a spec can, on the settled end state of a
 *  reply sent while the toggle is on.
 *
 *  Desktop only. The same rule runs on mobile, but the mobile header's own
 *  scroll compensation is a second writer over the same offset and the
 *  assertion would be about it rather than about the resize rule. */
test.describe('Transcript scroll belongs to the reader (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a send rides nothing, the toggle rides, and taking it back moves nobody', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    // A short viewport so a modest transcript overflows: with no scroll capacity
    // the assertions are vacuous.
    await page.setViewportSize({ width: 1280, height: 400 });
    // Nothing may ride until the toggle is pressed below, and the seed ships
    // armed. The test after this one is the half that covers the default.
    await disarmFollowSeed(page);
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
    const growLastTurn = () => tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const last = turns[turns.length - 1] as HTMLElement;
      const grown = document.createElement('div');
      grown.style.height = '600px';
      last.appendChild(grown);
    });

    // The first message in a brand-new thread. The submit holds them at the live
    // edge until the agent draws, which on a thread that IS this turn is barely
    // anywhere: far enough to bring the status row into view (ADR 0080). Then it
    // lets go, and the 40-line reply grows past the fold BELOW them, with the
    // chevron their way down. This used to end on the answer's last line.
    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(100);
    await expect.poll(atBottom).toBe(false);
    // Not merely short of the bottom: still up at the turn's own opening, a
    // small fraction of the way down, rather than carried through the reply.
    const opening = await tc.evaluate(el => el.scrollTop);
    const reach = await tc.evaluate(el => el.scrollHeight - el.clientHeight);
    expect(opening).toBeLessThan(reach / 4);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);

    // A send into that same thread DOES move them, once: the reader is above the
    // fold now, and what they write lands below it, so the landing takes them to
    // their own turn. It ARMS nothing, though, so the reply that streams in
    // afterwards leaves them exactly there and the chevron is their way down.
    await sendMessage(page, 'List the numbers from 1 to 20, one per line, and nothing else.');
    await waitForResponse(page);
    const landed = await tc.evaluate(el => el.scrollTop);
    expect(landed).toBeGreaterThan(50);
    await expect.poll(atBottom).toBe(false);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);
    // Not merely short of the bottom: unmoved since the landing, across
    // everything that arrived after it.
    await growLastTurn();
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(landed);

    // THE ONE STANDING ASK. The toggle takes them to the live edge, and unlike
    // the chevron it stays pressed, which is the visible difference between a
    // journey and a mode. One button cannot be go-there, stay-here and
    // stop-staying at once, which is why these are two controls.
    const toggle = page.locator('button[data-role="follow-live-edge"]:visible').first();
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    // And it CARRIES, which the send deliberately did not: the whole of this
    // reply arrives with the reader riding it, and they end on the live edge
    // rather than a screen above it. Asserted on the settled end state, because
    // that is what discriminates: a follow that armed but never wrote would
    // leave them where the submit's glide put them, a reply behind.
    await sendMessage(page, 'List the numbers from 1 to 30, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    // Past the old pin window (500ms) with room to spare, so the grow below lands
    // in the world the bug lived in: no suppression left, no render, no gesture.
    // The thread is IDLE, and an idle thread carries nobody who SCROLLED AWAY.
    // This reader never left the edge, and ADR 0064's other half is theirs: the
    // app's own rendering must not slide the edge out from under them. So the
    // grow keeps them on it rather than stranding them a screen above.
    await page.waitForTimeout(1500);
    await growLastTurn();
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');

    // Taking the ride back writes NO scroll: turning the follow off means "leave
    // me where I am reading", not "put me back where I was".
    const riding = await tc.evaluate(el => el.scrollTop);
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(riding);

    // And with the ride off, the next grow leaves them behind: that is the
    // difference the toggle makes, and the chevron is their way back down.
    await growLastTurn();
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(riding);
    await expect.poll(atBottom).toBe(false);

    // The chevron reaches the TRUE bottom of the grown content, and arms nothing.
    await page.locator('button.scroll-to-bottom.visible').click();
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  });

  /** The DEFAULT, which is the half the journey above cannot see. That test
   *  disarms the seed on purpose, so nothing in it says what a device that has
   *  pressed nothing does.
   *
   *  The seed ships ARMED, so this context rides with no press behind it. Only
   *  a disarm press turns it off, and there has been none. */
  test('a device that has never pressed the toggle rides a brand-new thread', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    await page.setViewportSize({ width: 1280, height: 400 });
    // Deliberately NOT disarmed: an untouched context IS the case under test.
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const toggle = page.locator('button[data-role="follow-live-edge"]:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);

    // The compose view has no transcript to describe, so the toggle shows the
    // SEED there. Lit before anything is sent is the whole of the default.
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');

    // And it CARRIES on the thread that compose becomes. The reply lands the
    // reader on the live edge. The same send leaves an unarmed reader up at the
    // turn's opening, a small fraction of the way down.
    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(100);
    await expect.poll(atBottom).toBe(true);
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
  });
});
