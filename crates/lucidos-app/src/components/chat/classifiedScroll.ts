import { classifyScroll, type ScrollClass } from './scrollState';

type ScrollListener = (scroll: ScrollClass, event: ScrollClass) => void;
type EngineListener = (scroll: ScrollClass) => ScrollClass;

interface Registration<L> { listener: L }

interface ContainerListeners {
  engine: Registration<EngineListener> | null;
  consumers: Set<Registration<ScrollListener>>;
  dispatch: () => void;
}

const _containers = new WeakMap<HTMLElement, ContainerListeners>();

function tell<R>(
  listener: (scroll: ScrollClass, event: ScrollClass) => R, scroll: ScrollClass, event: ScrollClass,
): R | undefined {
  try {
    return listener(scroll, event);
  } catch (error) {
    if (typeof reportError === 'function') reportError(error);
    else setTimeout(() => { throw error; }, 0);
    return undefined;
  }
}

function listenersOf(el: HTMLElement): ContainerListeners {
  let entry = _containers.get(el);
  if (!entry) {
    const created: ContainerListeners = {
      engine: null,
      consumers: new Set(),
      dispatch: () => {
        const event = classifyScroll(el);
        const scroll = created.engine ? tell(created.engine.listener, event, event) ?? event : event;
        for (const consumer of Array.from(created.consumers)) {
          if (created.consumers.has(consumer)) tell(consumer.listener, scroll, event);
        }
      },
    };
    entry = created;
    _containers.set(el, entry);
    el.addEventListener('scroll', entry.dispatch, { passive: true });
  }
  return entry;
}

function dropIfUnheard(el: HTMLElement, entry: ContainerListeners) {
  if (entry.engine || entry.consumers.size > 0) return;
  el.removeEventListener('scroll', entry.dispatch);
  if (_containers.get(el) === entry) _containers.delete(el);
}

export function onClassifiedScroll(el: HTMLElement, listener: ScrollListener): () => void {
  const entry = listenersOf(el);
  const registration = { listener };
  entry.consumers.add(registration);
  return () => {
    entry.consumers.delete(registration);
    dropIfUnheard(el, entry);
  };
}

export function onClassifiedScrollAsEngine(el: HTMLElement, listener: EngineListener): () => void {
  const entry = listenersOf(el);
  const registration = { listener };
  entry.engine = registration;
  return () => {
    if (entry.engine === registration) entry.engine = null;
    dropIfUnheard(el, entry);
  };
}
