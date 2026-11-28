
import { isTauri } from './platform';
import { setWindowTitle } from './tauri';

const PRODUCT = 'Lucidos';

export function documentTitle(workspace: string, unread: number): string {
  const name = workspace.trim();
  const base = name ? `${PRODUCT} - ${name}` : PRODUCT;
  return unread > 0 ? `(${unread}) ${base}` : base;
}

export function nativeWindowTitle(workspace: string): string {
  return workspace.trim() || PRODUCT;
}

let applied = '';

let queue: Promise<void> = Promise.resolve();

export function resetWindowTitlePush(): void {
  applied = '';
  queue = Promise.resolve();
}

export function pushNativeWindowTitle(workspace: string): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  const title = nativeWindowTitle(workspace);
  queue = queue.then(() => {
    if (title === applied) return;
    return setWindowTitle(title).then(
      () => {
        applied = title;
      },
      (e: unknown) => console.warn('[window] title not applied', e),
    );
  });
  return queue;
}
