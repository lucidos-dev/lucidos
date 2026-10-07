import { Component, createRef, type ComponentChildren, type RefObject } from 'preact';
import type { Signal } from '@preact/signals';
import type { DrawerView } from '../../store/store';
import { navCoverFuseMs } from '../shared/NavigationCover';

/** A drawing of the list as it looked when its status filter changed. Each dip
 *  takes a new `id`, even when it carries the same `node` over. */
export type LeavingDrawing = { id: number; node: HTMLElement; scrollTop: number };

let lastDrawingId = 0;

/** Attributes that give a row an identity. The drawing must not answer a row
 *  lookup, a FLIP pass or `aria-activedescendant`. */
const IDENTITY_ATTRIBUTES = ['id', 'data-thread-nav', 'data-flip-id'];

const DRAWING_CLASS = 'thread-view-drawing';
const DRAWING_LIST_CLASS = 'thread-view-drawing-list';

function isShowing(el: Element): boolean {
  return getComputedStyle(el).visibility === 'visible';
}

/** A drawing of `list` as the user sees it, or null when it is not what the
 *  user sees. A drawing already on screen carries itself over instead
 *  (`LeavingViewDrawing`), so a reversal never jumps to the list under it. */
export function drawLeavingList(list: HTMLElement | null): LeavingDrawing | null {
  if (!list?.firstElementChild || !isShowing(list)) return null;
  const showing = list.parentElement?.querySelector(`:scope > .${DRAWING_CLASS}`);
  if (showing && isShowing(showing)) return null;
  const node = list.cloneNode(true) as HTMLElement;
  node.className = DRAWING_LIST_CLASS;
  node.inert = true;
  for (const el of [node, ...node.querySelectorAll('*')]) {
    for (const name of IDENTITY_ATTRIBUTES) el.removeAttribute(name);
  }
  // A row in flight is drawn at rest. A clone carries no Web Animation, so its
  // flying copy would sit frozen at the flight's start.
  for (const portal of node.querySelectorAll('.flip-portal')) portal.replaceChildren();
  return { id: ++lastDrawingId, node, scrollTop: list.scrollTop };
}

/** Draws the list when its status filter changes. It wraps the list's content,
 *  because `getSnapshotBeforeUpdate` runs after render and before the content's
 *  DOM changes: the one moment the leaving view still exists. */
export class LeavingViewSnapshot extends Component<{
  view: DrawerView;
  list: RefObject<HTMLElement>;
  drawing: Signal<LeavingDrawing | null>;
  children?: ComponentChildren;
}> {
  getSnapshotBeforeUpdate(previous: Readonly<{ view: DrawerView }>): LeavingDrawing | null {
    return previous.view === this.props.view ? null : drawLeavingList(this.props.list.current);
  }

  componentDidUpdate(_props: unknown, _state: unknown, drawn: LeavingDrawing | null): void {
    if (drawn) this.props.drawing.value = drawn;
  }

  render() {
    return this.props.children;
  }
}

/** The leaving list, held over the pane until the dip's midpoint, where the
 *  CSS hides it (drawer.css). It plays the part the closing filter panel plays
 *  on the panel path. A drawing, not a view: its content is `inert` and the
 *  frame `aria-hidden`. The frame itself takes a tap while it shows, so a tap
 *  never reaches the arriving list before the user can see it.
 *
 *  Keyed on the drawing, so each dip mounts a fresh element whose animation
 *  starts with the fresh navigation cover's. A new dip (`swapKey`) that finds it
 *  still showing re-keys it, so it holds until the NEW midpoint. It unmounts on
 *  the cover's own fuse. */
export class LeavingViewDrawing extends Component<{
  swapKey: string;
  drawing: Signal<LeavingDrawing | null>;
}> {
  private frame = createRef<HTMLDivElement>();
  /** The drawing the last render committed a frame for, and the one shown. */
  private rendered: LeavingDrawing | null = null;
  private shown: LeavingDrawing | null = null;
  private fuse: ReturnType<typeof setTimeout> | undefined;

  getSnapshotBeforeUpdate(previous: Readonly<{ swapKey: string }>): LeavingDrawing | null {
    const frame = this.frame.current;
    const node = frame?.firstElementChild as HTMLElement | null | undefined;
    if (previous.swapKey === this.props.swapKey || !frame || !node || !isShowing(frame)) return null;
    return { id: ++lastDrawingId, node, scrollTop: node.scrollTop };
  }

  componentDidMount(): void {
    this.show();
  }

  componentDidUpdate(_props: unknown, _state: unknown, carried: LeavingDrawing | null): void {
    if (carried) this.props.drawing.value = carried;
    this.show();
  }

  componentWillUnmount(): void {
    clearTimeout(this.fuse);
    this.props.drawing.value = null;
  }

  /** Puts a newly mounted frame's drawing in it and arms that drawing's fuse. */
  private show(): void {
    const current = this.rendered;
    if (current === this.shown) return;
    this.shown = current;
    clearTimeout(this.fuse);
    if (!current) return;
    this.frame.current?.append(current.node);
    current.node.scrollTop = current.scrollTop;
    const { drawing } = this.props;
    this.fuse = setTimeout(() => {
      if (drawing.peek() === current) drawing.value = null;
    }, navCoverFuseMs('dip'));
  }

  render() {
    const current = this.props.drawing.value;
    this.rendered = current;
    if (!current) return null;
    return <div key={current.id} ref={this.frame} class={DRAWING_CLASS} aria-hidden="true" />;
  }
}
