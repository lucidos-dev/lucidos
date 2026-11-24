
import { serveHost } from './_bridge';
import { capture } from './capture';
import { serveFind } from './find';

export function installHostOps(): void {
  serveHost('capture', (args) => capture((args as { format?: string } | null)?.format));

  serveHost('hash', (args) => {
    const fragment = (args as { fragment?: string })?.fragment ?? '';
    const target = new URL(location.href);
    target.hash = fragment;
    if (target.href === location.href) return false;
    location.replace(target.href);
    return true;
  });

  serveHost('find', serveFind);
}
