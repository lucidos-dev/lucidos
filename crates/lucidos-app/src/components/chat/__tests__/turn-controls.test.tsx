/** The turn's three controls, which live in the response HEADER.
 *
 *  Two of them were text links at the top of the response body ("More" /
 *  "Less" and "Show steps" / "Hide steps"); the third was a click anywhere on
 *  the header row, announced by nothing but a cursor. What the move bought is
 *  pinned here property by property, because each is easy to lose in a later
 *  edit: the group sits between the executor and the meta cluster, every
 *  control is a real button, and the one control whose effect stops at this
 *  turn says so in its label.
 *
 *  How each states its state is the split that took the most goes. The PAIR
 *  keeps a fixed glyph and brightens (`FullResponseIcon` records why a moving
 *  glyph was wrong for them). The COLLAPSE control is the mirror image: its
 *  glyph flips and its brightness never moves. Both halves and the reasons are
 *  pinned below, since either one drifting silently re-breaks a reported bug.
 */
import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ResponsePanel, turnControls } from '../chat-exchange-parts';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

function children(node: ComponentChildren): ComponentChildren[] {
  const kids = (node as AnyVNode)?.props?.children;
  return Array.isArray(kids) ? kids : [kids];
}

function findByClass(node: ComponentChildren, cls: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByClass(c, cls);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (String(v.props?.class ?? '').split(/\s+/).includes(cls)) return v;
  return findByClass(v.props?.children, cls);
}

function findByRole(node: ComponentChildren, role: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByRole(c, role);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.props?.['data-role'] === role) return v;
  return findByRole(v.props?.children, role);
}

const noop = () => {};

const controls = (over: Partial<Parameters<typeof turnControls>[0]> = {}) => turnControls({
  detailsOn: false,
  stepsOn: false,
  collapsed: false,
  collapsible: true,
  onToggleDetails: noop,
  onToggleSteps: noop,
  onToggleCollapsed: noop,
  ...over,
});

const ROLES = ['toggle-details', 'toggle-steps', 'toggle-collapsed'];

/** Every combination of the three states, so a property claimed "in every
 *  state" is checked in all eight rather than in the two anyone thinks of. */
const EVERY_STATE = [false, true].flatMap((detailsOn) =>
  [false, true].flatMap((stepsOn) =>
    [false, true].map((collapsed) => ({ detailsOn, stepsOn, collapsed }))));

describe('turnControls', () => {
  it('renders all three controls in every state', () => {
    // Fixed chrome, not a function of this turn's events. Two of the three flip
    // a per-user setting spanning the transcript, and the third is the only way
    // to unfold a folded turn, so hiding the group while collapsed would strand
    // it. Conditional controls (which is what the text links were) made the
    // group jump between turns and left holes in a column of identical headers.
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        expect(findByRole(controls(state), role), `${role} ${JSON.stringify(state)}`).not.toBeNull();
      }
    }
  });

  it('makes every control a real button', () => {
    // Keyboard reachability and the right semantics for a toggle, and what
    // keeps a click off the INITIATOR header's collapse handler, which does
    // still fold on a row click (`handlePanelHeaderClick` skips `button, a`).
    for (const role of ROLES) {
      const btn = findByRole(controls(), role)!;
      expect(btn.type, role).toBe('button');
      expect(btn.props.type, role).toBe('button');
    }
  });

  it('states each control in aria-pressed', () => {
    // The CSS keys the brightened "on" look off this same attribute, so a
    // control that stops reporting its state also stops looking like it has one.
    const off = controls();
    for (const role of ROLES) expect(findByRole(off, role)!.props['aria-pressed'], role).toBe(false);

    const on = controls({ detailsOn: true, stepsOn: true, collapsed: true });
    for (const role of ROLES) expect(findByRole(on, role)!.props['aria-pressed'], role).toBe(true);
  });

  it('names every control, since none of them carries visible text', () => {
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        const btn = findByRole(controls(state), role)!;
        expect(String(btn.props['aria-label'] ?? ''), role).not.toBe('');
        // Desktop hover help. The rule bans native `title` tooltips outright,
        // so this is the only way the icon explains itself before a click.
        expect(btn.props['data-tooltip'], role).toBe(btn.props['aria-label']);
      }
    }
  });

  it('says "this turn" on the one control whose effect stops here', () => {
    // The scope split is the thing a reader has to get right: two of these
    // change every turn in the transcript, one folds the turn it sits on. A gap
    // in the row hints at it; the label is what states it, so the collapse
    // control names the turn and the other two never do.
    const label = (role: string, state = {}) => String(findByRole(controls(state), role)!.props['aria-label']);
    expect(label('toggle-collapsed')).toMatch(/this turn/i);
    expect(label('toggle-collapsed', { collapsed: true })).toMatch(/this turn/i);
    expect(label('toggle-details')).not.toMatch(/this turn/i);
    expect(label('toggle-steps')).not.toMatch(/this turn/i);
  });

  it('disables the collapse control on a turn with no body to fold', () => {
    // `canCollapse` is false while a panel is only a status line. The store
    // would take the fold and hold it, so an enabled control there reads dead
    // on the click and then folds the turn the moment its first content lands.
    // The other two act on a transcript-wide setting, so they never disable.
    const none = controls({ collapsible: false });
    expect(findByRole(none, 'toggle-collapsed')!.props.disabled).toBe(true);
    expect(findByRole(none, 'toggle-details')!.props.disabled).toBeUndefined();
    expect(findByRole(none, 'toggle-steps')!.props.disabled).toBeUndefined();
    expect(findByRole(controls(), 'toggle-collapsed')!.props.disabled).toBe(false);
  });

  it('draws the transcript-wide pair with one fixed glyph, whatever the state', () => {
    // The full-response control started as an unfold/fold pair, on the theory
    // that a fixed glyph cannot say which way the next click goes. The two
    // forms shared a box but not their ink, so the mark visibly changed size
    // on every click while the body under it was also moving, which read as
    // the layout dancing. `aria-pressed` plus the brightness rule carry the
    // state for these two instead.
    for (const role of ['toggle-details', 'toggle-steps']) {
      const glyph = (state: object) => findByRole(controls(state), role)!.props.children as AnyVNode;
      const base = glyph({});
      for (const state of EVERY_STATE) {
        expect(glyph(state).type, role).toBe(base.type);
        expect(glyph(state).props, role).toEqual(base.props);
      }
    }
  });

  /** Invoke the icon with the props the CONTROL handed it, so these cover the
   *  control forwarding its state as well as the icon drawing on it. */
  const arrows = (collapsed: boolean) => {
    const icon = findByRole(controls({ collapsed }), 'toggle-collapsed')!.props.children as AnyVNode;
    const draw = icon.type as (props: Record<string, unknown>) => AnyVNode;
    return (draw(icon.props).props.children as AnyVNode[]).map((p) => {
      const n = String(p.props.points).trim().split(/\s+/).map(Number);
      return { wings: [n[1], n[5]], apex: n[3], left: n[0], right: n[4] };
    });
  };

  it('turns the collapse arrows around, so direction says which way the click goes', () => {
    // The one control whose glyph moves, because it is the one with no colour
    // to move: it is exempt from the brightness rule (styles/chat/response.css)
    // since bright-means-folded would invert the pair's own bright-means-more
    // 0.125rem away, and would restate a fold the `⋯` stub already shows.
    //
    // Assert the MEANING, not just that something changed. Converging (each
    // apex between the two wings, vertically) is "Collapse this turn";
    // diverging is "Expand this turn". The tooltip says the same words, so a
    // silent swap of the two point sets would put the mark and its own label
    // in contradiction, which nothing else here would catch.
    const [top, bottom] = arrows(false);
    expect(top.apex, 'expanded: top arrowhead points down').toBeGreaterThan(Math.max(...top.wings));
    expect(bottom.apex, 'expanded: bottom arrowhead points up').toBeLessThan(Math.min(...bottom.wings));

    const [ctop, cbottom] = arrows(true);
    expect(ctop.apex, 'collapsed: top arrowhead points up').toBeLessThan(Math.min(...ctop.wings));
    expect(cbottom.apex, 'collapsed: bottom arrowhead points down').toBeGreaterThan(Math.max(...cbottom.wings));
  });

  it('keeps the mark small, which is the half of the old complaint that still binds', () => {
    // `3e8c8f6f6` removed a moving glyph for TWO reasons, and only one of them
    // is answered by this control having no other state cue. The other was
    // plain size: that mark's ink spanned 22 of 24 units against the log
    // glyph's 12, so it "towered over both the label and its neighbour".
    //
    // Note what this does NOT claim. The banned pair was ALSO two arrowheads
    // each reflected about its own midline, with the same span, the same
    // summed segment length and the same stroke count across its two forms
    // (x 5 to 19, y 2 to 22, 4 segments of sqrt(74)). So none of those
    // properties distinguishes the permitted case from the prohibited one, and
    // a test asserting them would pass on the banned coordinates and guard
    // nothing. The envelope is the one thing that genuinely differs, so it is
    // the one thing pinned: keep this mark nearer its neighbours than the mark
    // that was removed for being too big.
    //
    // The ceiling started at 18 and came down to 14 when the gap between the
    // two arrowheads was reported as too much air, which had the mark standing
    // half again as tall as the step-log glyph (12) beside it. 14 is therefore
    // the size someone asked for rather than a round number, and the reason it
    // is an equality in spirit: drifting back UP re-opens the complaint.
    const BANNED_ENVELOPE = 22; // y 2..22 plus a 1-unit round cap each end.
    const CAP = 2;              // stroke-width 2, so 1 unit of cap top and bottom.
    for (const collapsed of [false, true]) {
      const ys = arrows(collapsed).flatMap((a) => [...a.wings, a.apex]);
      const envelope = Math.max(...ys) - Math.min(...ys) + CAP;
      expect(envelope, `collapsed=${collapsed}`).toBeLessThan(BANNED_ENVELOPE);
      expect(envelope, `collapsed=${collapsed}`).toBeLessThanOrEqual(14);
    }
  });

  it('keeps a channel between the two arrowheads, whichever way they point', () => {
    // The floor to the ceiling above, and the two are one decision: the
    // envelope is depth + gap + depth + caps, so a test that only caps the
    // total invites the next slimming pass to buy depth out of the gap. That
    // is the direction the original smudge lies in, and it is the 14px box a
    // plain desktop root gives this that it has to survive, which is not what
    // anyone is looking at while they nudge the coordinates.
    //
    // 4 units of gap is 2 of daylight once the 2-unit stroke is taken off,
    // ~1.2px in that box, against the banned three-mark version's ~0.9px. That
    // is the whole margin, so it is worth being exact about how small it is:
    // the banned mark pinched at a single x as well, so the difference is not
    // its shape, it is that 2 units clears a device pixel where 1.5 did not,
    // that there is one pinch here rather than two, and that this one opens to
    // 10 units of daylight at its widest against that one's 5.5.
    //
    // Sampled at the apexes and at the wing tips because those are the two
    // extremes, and the pair is a mirror image about its own midline: the gap
    // runs linearly between them, so its minimum is at one end or the other.
    // Converging pinches at the apexes, diverging at the wing tips.
    const waist = (a: ReturnType<typeof arrows>) => {
      const [top, bottom] = a;
      return Math.min(bottom.apex - top.apex, Math.min(...bottom.wings) - Math.max(...top.wings));
    };
    for (const collapsed of [false, true]) {
      expect(waist(arrows(collapsed)), `collapsed=${collapsed}`).toBeGreaterThanOrEqual(4);
    }
  });

  it('moves the box and the weight not at all, only the direction', () => {
    // Weaker than it looks (see the envelope test above for why), but still
    // worth holding: whatever the next redraw does, the two forms must not
    // differ in extent or in how much stroke is inside it, or the flip starts
    // costing a size change ON TOP of the direction change.
    const extent = (a: ReturnType<typeof arrows>) => {
      const ys = a.flatMap((p) => [...p.wings, p.apex]);
      const xs = a.flatMap((p) => [p.left, p.right]);
      return [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    };
    /** Summed segment length, to three decimals. */
    const ink = (a: ReturnType<typeof arrows>) => Number(a.reduce((total, p) => total
      + Math.hypot((p.left + p.right) / 2 - p.left, p.apex - p.wings[0])
      + Math.hypot(p.right - (p.left + p.right) / 2, p.wings[1] - p.apex), 0).toFixed(3));

    expect(extent(arrows(false))).toEqual(extent(arrows(true)));
    expect(ink(arrows(false))).toBe(ink(arrows(true)));
    expect(arrows(false).length).toBe(arrows(true).length);
  });

  it('reads the three controls as three different actions', () => {
    // Each label changes with its own state, and no two ever coincide: three
    // icons whose hover text agrees are unusable without clicking one.
    const label = (role: string, state = {}) => findByRole(controls(state), role)!.props['aria-label'];
    const on = { detailsOn: true, stepsOn: true, collapsed: true };
    for (const role of ROLES) expect(label(role), role).not.toBe(label(role, on));
    expect(new Set(ROLES.map((r) => label(r))).size).toBe(ROLES.length);
    expect(new Set(ROLES.map((r) => label(r, on))).size).toBe(ROLES.length);
  });
});

describe('ResponsePanel', () => {
  const panel = (slot: ComponentChildren) => ResponsePanel({
    executor: { icon: null, label: 'Claude Code' },
    controls: slot,
    status: null,
    timestamp: '14:32',
    collapsed: false,
    hasBody: true,
    children: null,
  });

  it('puts the controls between the executor and the meta cluster', () => {
    const header = findByClass(panel(controls()), 'response-header')!;
    const positions = children(header).map((c) => {
      const cls = String((c as AnyVNode)?.props?.class ?? '');
      if (cls.includes('response-executor')) return 'executor';
      if (cls.includes('response-controls')) return 'controls';
      if (cls.includes('response-meta')) return 'meta';
      return null;
    }).filter(Boolean);
    expect(positions).toEqual(['executor', 'controls', 'meta']);
  });

  it('leaves the response header inert, since the collapse control owns folding', () => {
    // The row used to swallow a click to fold the turn, with nothing but a
    // cursor to announce it, under three buttons that each mean something else.
    // A handler here would also fire for any click that misses a button by a
    // pixel, which is most of the row.
    const header = findByClass(panel(controls()), 'response-header')!;
    expect(header.props.onClick).toBeUndefined();
    expect(String(header.props.class)).toBe('response-header');
  });
});
