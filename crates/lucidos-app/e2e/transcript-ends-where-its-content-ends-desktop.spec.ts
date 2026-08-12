import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

/** The transcript reserves NOTHING under its newest turn, so it ends exactly
 *  where its content ends, and a submit brings the turn the reader acted on as
 *  far up the viewport as that real content allows.
 *
 *  This has to be a browser test, and it is the only thing here that can be. The
 *  unit suite (`components/chat/__tests__/scroll-follow-the-live-edge.test.ts`)
 *  drives a fake container whose `scrollHeight` is set by hand, so it can prove
 *  the landing aims at the right number and cannot prove anything about what the
 *  layout actually is. Air below the last turn is layout.
 *
 *  It replaces a spec that asserted the opposite. Between 2026-08-11 and
 *  2026-08-12 the transcript reserved a TAIL ROOM, one viewport of `min-height`
 *  on its last turn, so that a submit could rest that turn's top on the landing
 *  line with the reply growing into the room. It was reserved air, and air below
 *  the last turn misreports how much thread there is: a reader riding the live
 *  edge was carried into it and lost the running reply off the top of the screen,
 *  withholding it from a rider made the layout depend on the follow flag so any
 *  failure to re-arm showed a screenful of blank, and it appeared mid-turn either
 *  way because a queued follow-up only qualified once the agent picked it up.
 *
 *  So what is pinned here is the absence, plus the two things a reader still gets
 *  without it: their message on screen, and the agent status line under it.
 *
 *  Desktop only, matching `thread-scroll-belongs-to-the-reader.spec.ts`: the same
 *  rule runs on mobile, but the mobile header's own scroll compensation is a
 *  second writer over the same offset and the assertion would be about it. */
test.describe('The transcript ends where its content ends (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('reserves no air under the newest turn, and still shows what was sent', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    // SHORT, so a couple of mock turns overflow it and the transcript is really
    // scrollable. The spec this replaces wanted the opposite (a viewport tall
    // enough that the reserved room dwarfed a turn); with nothing reserved, an
    // unscrollable thread would make every assertion here vacuously true.
    await page.setViewportSize({ width: 1280, height: 600 });
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();

    /** Everything judged here, measured off the LAST turn.
     *
     *  The trailing space is differenced against the transcript's LAST CHILD
     *  rather than its last `.chat-exchange`, and computed from rects rather
     *  than `offsetTop`. Both matter: the transcript may legitimately render
     *  something after the newest turn, so measuring off the turn would call
     *  that sibling "air" and fail for a reason this spec is not about, and
     *  `offsetTop` is relative to the offset PARENT, which is only the
     *  transcript while it stays positioned. The reserved air this spec exists
     *  to keep out is caught by the `min-height` check beside it either way. */
    const geometry = () => tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const last = turns[turns.length - 1] as HTMLElement | undefined;
      if (!last) return null;
      const top = el.getBoundingClientRect().top;
      const header = last.querySelector('.response-header') as HTMLElement | null;
      const initiator = last.querySelector('.initiator-panel') as HTMLElement | null;
      const tail = el.lastElementChild as HTMLElement;
      const tailBottom = tail.getBoundingClientRect().bottom - top + el.scrollTop;
      return {
        /** Space after the last child in the scrolled content. The transcript's
         *  own bottom padding is all there may ever be; anything more is air. */
        trailing: el.scrollHeight - tailBottom,
        paddingBottom: parseFloat(getComputedStyle(el).paddingBottom) || 0,
        /** Named in the failure message, because "what is at the bottom of the
         *  transcript" is the one thing a bare number cannot tell you. */
        tailClass: tail.className,
        /** Nothing may floor the last turn's height. */
        minHeight: getComputedStyle(last).minHeight,
        /** The reader's own message, and the agent status line, in the
         *  transcript's coordinates. */
        messageTop: initiator ? initiator.getBoundingClientRect().top - top : null,
        messageBottom: initiator ? initiator.getBoundingClientRect().bottom - top : null,
        statusLineBottom: header ? header.getBoundingClientRect().bottom - top : null,
        clientHeight: el.clientHeight,
        scrollable: el.scrollHeight > el.clientHeight + 10,
      };
    });

    /** The invariant, asserted at every stage rather than once at the end: the
     *  air used to arrive at a different moment for each reader (on the first
     *  paint of a reply, and on the agent picking a queued message up), so a
     *  single check at rest is exactly the shape that missed it. */
    const expectNoAir = async (when: string) => {
      // Only the PRECONDITION is polled, never the claim. A send on a brand-new
      // thread has no turn to measure for a beat (the optimistic row arrives a
      // frame or more after the composer clears), so measuring at once reads a
      // transcript that is not there yet. Polling the assertion itself instead
      // would be the wrong shape: transient air is exactly what was reported, and
      // a poll that waits for the good answer would let it through.
      await expect.poll(async () => (await geometry()) !== null,
        { message: `${when}: no turn rendered to measure` }).toBe(true);
      const g = (await geometry())!;
      // ONLY WHILE THE TRANSCRIPT OVERFLOWS, because otherwise `scrollHeight` is
      // clamped to `clientHeight` and the space under the last turn is unused
      // VIEWPORT rather than reserved air. That is not a loophole: a reservation
      // is a screenful, so it forces the overflow itself and lands squarely in
      // the branch that is checked. (Learned the hard way: this spec first
      // reported 189px of "air" on a two-turn thread that did not yet scroll.)
      if (g.scrollable) {
        // Within a pixel: `scrollHeight` is an integer while the rect and the
        // padding it is differenced against are fractional at this root size, so
        // exact equality fails on the rounding alone.
        expect(
          Math.abs(g.trailing - g.paddingBottom),
          `${when}: ${Math.round(g.trailing)}px under the last child (.${g.tailClass}), against ${Math.round(g.paddingBottom)}px of padding`,
        ).toBeLessThanOrEqual(1);
      }
      // Unconditional, and the sharper half: a reserved length here is the room
      // itself, whether or not the thread is long enough to show it yet.
      // `min-height: 0px` or `auto`, never a length.
      expect(['0px', 'auto'], `${when}: the last turn is floored`).toContain(g.minHeight);
    };

    await sendMessage(page, 'Reply with exactly the word: one');
    await expectNoAir('the moment the first message is sent');
    await waitForResponse(page);
    await expectNoAir('the first reply is in');

    await sendMessage(page, 'Reply with exactly the word: two');
    await expectNoAir('the moment the second message is sent');
    await waitForResponse(page);
    await expectNoAir('the second reply is in');

    // The thread really does overflow, so the assertions above were about a
    // container that had somewhere to put air.
    const landed = (await geometry())!;
    expect(landed.scrollable, 'the thread never became scrollable').toBe(true);

    // AND THE READER GOT WHAT THE LANDING IS FOR. Their message is on screen and
    // the agent status line is under it, which is the half of the top landing
    // that survives without anything reserved: the submit brings the turn as far
    // up as the real content allows, and for a fresh turn that is the bottom of
    // the screen rather than the landing line.
    expect(landed.messageTop).not.toBeNull();
    expect(landed.messageTop!, 'the sent message is above the viewport').toBeGreaterThanOrEqual(-1);
    expect(landed.messageBottom!, 'the sent message is below the fold').toBeLessThanOrEqual(landed.clientHeight);
    expect(landed.statusLineBottom).not.toBeNull();
    expect(landed.statusLineBottom!, 'the status line is below the fold').toBeLessThanOrEqual(landed.clientHeight);

    // GROWING THE TURN adds no air either, which is the case a `min-height` used
    // to answer by self-collapsing and now needs no answer at all.
    await tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const grown = document.createElement('div');
      grown.style.height = '2000px';
      (turns[turns.length - 1] as HTMLElement).appendChild(grown);
    });
    // ONE measurement per poll round: two `geometry()` calls would difference
    // two different observations of a container that is still settling.
    await expect.poll(async () => {
      const g = (await geometry())!;
      return g.trailing - g.paddingBottom;
    }, { message: 'growing the turn left air under it' }).toBeLessThanOrEqual(1);
  });
});
