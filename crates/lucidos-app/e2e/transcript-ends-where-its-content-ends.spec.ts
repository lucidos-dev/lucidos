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
 *  EVERY PROJECT, desktop and mobile. It skipped mobile on the grounds that the
 *  mobile header's scroll compensation is a second writer over the same offset.
 *  That reasoning is about WHERE THE READER IS, and this spec measures LAYOUT:
 *  how much space sits under the last child. Whoever wrote the offset, air is
 *  air.
 *
 *  A TRANSCRIPT SHORTER THAN ITS PANE IS THE OTHER CASE, and it answers a
 *  different question. There is no air to reserve, because `scrollHeight` is
 *  clamped to `clientHeight`: the space under the last turn is unused viewport,
 *  and the scroll height does not lie about it. What is pinned there is that
 *  NOTHING MOVES. The turns start at the top of the feed. The transcript does
 *  not scroll at all. A turn already drawn stays where it was when the next one
 *  arrives.
 *
 *  That case spent one day resting its turns on the BOTTOM instead, to close
 *  the unused viewport. Bottom-anchored, every new turn pushed everything above
 *  it up the screen, so content the reader had read travelled before there was
 *  a page of it. Reverted the same day.
 *  Plan: docs/plans/2026-09-17-a-short-transcript-holds-still.md */
test.describe('The transcript ends where its content ends', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('reserves no air, holds a short thread still, and still shows what was sent', async ({ page }) => {
    // SHORT, so a couple of mock turns overflow it and the transcript is really
    // scrollable. The spec this replaces wanted the opposite (a viewport tall
    // enough that the reserved room dwarfed a turn); with nothing reserved, an
    // unscrollable thread would make every assertion here vacuously true.
    // A mobile project already ships a short viewport, and resizing one would
    // measure a shape no phone has.
    if (!isMobileViewport(page)) await page.setViewportSize({ width: 1280, height: 600 });
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();

    /** Everything judged here, measured off the LAST turn.
     *
     *  The trailing space is differenced against the feed's LAST CHILD rather
     *  than its last `.chat-exchange`, and computed from rects rather than
     *  `offsetTop`. Both matter. The feed may legitimately render something
     *  after the newest turn, and measuring off the turn would call that
     *  sibling "air". And `offsetTop` is relative to the offset PARENT, which
     *  is only the transcript while it stays positioned. The reserved air this
     *  spec exists to keep out is caught by the `min-height` check beside it
     *  either way. */
    const geometry = () => tc.evaluate((el) => {
      // MEASURED INSIDE THE FEED, never off the scroller. The scroller also
      // holds the mobile chrome. Its own first and last children are a header
      // spacer and the transcript's bottom padding, rather than turns.
      //
      // No fallback to the scroller when the feed is absent. A run that took
      // one read a chrome-sized inset and reported it as spare space handed to
      // the turns. Turns and their box render together, so "turns but no feed"
      // is a half-drawn frame and the poll below waits it out.
      const feed = el.querySelector('.thread-feed') as HTMLElement | null;
      const turns = feed ? feed.querySelectorAll('.chat-exchange') : [];
      const last = turns[turns.length - 1] as HTMLElement | undefined;
      if (!feed || !last) return null;
      const top = el.getBoundingClientRect().top;
      const header = last.querySelector('.response-header') as HTMLElement | null;
      const initiator = last.querySelector('.initiator-panel') as HTMLElement | null;
      const tail = feed.lastElementChild as HTMLElement;
      const tailBottom = tail.getBoundingClientRect().bottom - top + el.scrollTop;
      // The feed's FIRST child, for the reason its last one is used above. The
      // feed may render something before the oldest turn. Spare space is handed
      // to whatever is first, so measuring off the turn would read that
      // sibling's height as spare space.
      const head = feed.firstElementChild as HTMLElement;
      return {
        /** Space after the last child in the scrolled content. The transcript's
         *  own bottom padding is all there may ever be; anything more is air. */
        trailing: el.scrollHeight - tailBottom,
        paddingBottom: parseFloat(getComputedStyle(el).paddingBottom) || 0,
        /** Named in the failure message, because "what is at the bottom of the
         *  transcript" is the one thing a bare number cannot tell you. */
        tailClass: tail.className,
        /** How far the feed's first child sits below the feed's own top. Zero
         *  is the whole of the short case: the content starts where its box
         *  starts, so spare space handed to it shows up here, and it shrinks as
         *  the content grows. That shrinking IS the travel reported. */
        headInset: head.getBoundingClientRect().top - feed.getBoundingClientRect().top,
        /** How far the transcript can be scrolled. Zero is "there is no
         *  scroll", which is the state the rest below is asserted in. */
        overflow: el.scrollHeight - el.clientHeight,
        /** Nothing may floor the last turn's height. */
        minHeight: getComputedStyle(last).minHeight,
        /** The reader's own message, and the agent status line, in the
         *  transcript's coordinates. */
        messageTop: initiator ? initiator.getBoundingClientRect().top - top : null,
        messageBottom: initiator ? initiator.getBoundingClientRect().bottom - top : null,
        statusLineBottom: header ? header.getBoundingClientRect().bottom - top : null,
        clientHeight: el.clientHeight,
        scrollable: el.scrollHeight > el.clientHeight + 10,
        /** Where the mobile sticky title bar sits ON SCREEN, measured down from
         *  the scrollport's top, beside the header spacer it rides under. Null
         *  on desktop, where the row is `display: none`.
         *
         *  In VIEWPORT coordinates, deliberately. The row is `position: sticky`.
         *  With `scrollTop` added, a pinned row reads as far down the content as
         *  the reader has scrolled. The assertion would then fail on a scrolled
         *  thread that looks perfect.
         *
         *  Asserted rather than assumed: any alignment reaching the scroller
         *  hands spare space to every child of it, this band included, and it
         *  once landed 482px down the pane. */
        titleTop: (() => {
          const row = el.querySelector('.mobile-thread-title-row') as HTMLElement | null;
          if (!row || getComputedStyle(row).display === 'none') return null;
          return row.getBoundingClientRect().top - top;
        })(),
        spacerHeight: parseFloat(getComputedStyle(el, '::before').height) || 0,
      };
    });

    /** Did any round measure a transcript that does NOT overflow? The rounds
     *  below walk a thread from its first send, so the early ones are short of
     *  a pane and the later ones are not. Both cases matter and neither is
     *  scheduled, so the run says which it saw rather than assuming. */
    let sawUnscrollable = false;

    /** The invariant, asserted at every stage rather than once at the end: the
     *  air used to arrive at a different moment for each reader (on the first
     *  paint of a reply, and on the agent picking a queued message up), so a
     *  single check at rest is exactly the shape that missed it. */
    const expectNoAirAndNothingMoved = async (when: string) => {
      // Only the PRECONDITION is polled, never the claim. A send on a brand-new
      // thread has no turn to measure for a beat (the optimistic row arrives a
      // frame or more after the composer clears), so measuring at once reads a
      // transcript that is not there yet. Polling the assertion itself instead
      // would be the wrong shape: transient air is exactly what was reported, and
      // a poll that waits for the good answer would let it through.
      await expect.poll(async () => (await geometry()) !== null,
        { message: `${when}: no turn rendered inside a feed to measure` }).toBe(true);
      const g = (await geometry())!;
      if (!g.scrollable) sawUnscrollable = true;
      if (g.scrollable) {
        // AIR, and only where there is somewhere to put it. Without overflow
        // `scrollHeight` is clamped to `clientHeight`, so this difference
        // measures unused VIEWPORT instead and is always the padding.
        //
        // Within a pixel, because `scrollHeight` is an integer. The rect and
        // the padding differenced against it are fractional at this root size,
        // so exact equality fails on the rounding alone.
        expect(
          Math.abs(g.trailing - g.paddingBottom),
          `${when}: ${Math.round(g.trailing)}px under the last child (.${g.tailClass}), against ${Math.round(g.paddingBottom)}px of padding`,
        ).toBeLessThanOrEqual(1);
      } else {
        // NOTHING MOVES, which is the whole of the short case, and the inset is
        // the whole of the statement. Spare space handed to the turns sits
        // ABOVE them. Every new turn spends some of it, so the oldest turn
        // climbs the pane by the new one's height. Zero spare space is zero
        // travel, for every round and every source of rows.
        expect(
          g.headInset,
          `${when}: the feed's content starts ${Math.round(g.headInset)}px below the feed's own top`,
        ).toBeLessThanOrEqual(1);
        // And there is no scroll to be taken down either, which is the reader's
        // own words for it. A floor that outgrows the pane by a chrome's height
        // fails here rather than moving anybody.
        expect(
          g.overflow,
          `${when}: a thread short of the pane scrolls by ${Math.round(g.overflow)}px`,
        ).toBeLessThanOrEqual(1);
      }
      // Unconditional, and the sharper half: a reserved length here is the room
      // itself, whether or not the thread is long enough to show it yet.
      // `min-height: 0px` or `auto`, never a length.
      expect(['0px', 'auto'], `${when}: the last turn is floored`).toContain(g.minHeight);
      // The mobile title band stays at the TOP of the pane, never carried down
      // with the turns. At most the spacer's height: at rest it sits just under
      // it, and hide-on-scroll only ever slides it further up.
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

    // The thread really does overflow, so the assertions above were about a
    // container that had somewhere to put air.
    const landed = (await geometry())!;
    expect(landed.scrollable, 'the thread never became scrollable').toBe(true);

    // And it really did start SHORT of a pane, which is the case the void was
    // reported in. Both branches are covered by one walk, so a fixture change
    // that skipped the short one would silently drop half of this spec.
    expect(sawUnscrollable, 'no round measured a transcript short of the pane').toBe(true);

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
