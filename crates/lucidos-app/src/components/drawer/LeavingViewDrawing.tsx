import { Component, createRef, type ComponentChildren, type RefObject } from 'preact';
import type { Signal } from '@preact/signals';
import type { DrawerGrouping } from '../../store/store';
import { navCoverFuseMs } from '../shared/NavigationCover';

export type LeavingDrawing = { id: number; node: HTMLElement; scrollTop: number };

let lastDrawingId = 0;

const IDENTITY_ATTRIBUTES = ['id', 'data-thread-nav', 'data-flip-id'];

const DRAWING_CLASS = 'thread-view-drawing';
const DRAWING_LIST_CLASS = 'thread-view-drawing-list';

function isShowing(el: Element): boolean {
  return getComputedStyle(el).visibility === 'visible';
}

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
  for (const layer of node.querySelectorAll('.flip-departure-layer')) layer.remove();
  return { id: ++lastDrawingId, node, scrollTop: list.scrollTop };
}

export class LeavingViewSnapshot extends Component<{
  grouping: DrawerGrouping;
  list: RefObject<HTMLElement>;
  drawing: Signal<LeavingDrawing | null>;
  children?: ComponentChildren;
}> {
  getSnapshotBeforeUpdate(previous: Readonly<{ grouping: DrawerGrouping }>): LeavingDrawing | null {
    return previous.grouping === this.props.grouping ? null : drawLeavingList(this.props.list.current);
  }

  componentDidUpdate(_props: unknown, _state: unknown, drawn: LeavingDrawing | null): void {
    if (drawn) this.props.drawing.value = drawn;
  }

  render() {
    return this.props.children;
  }
}

export class LeavingViewDrawing extends Component<{
  swapKey: string;
  drawing: Signal<LeavingDrawing | null>;
}> {
  private frame = createRef<HTMLDivElement>();
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
