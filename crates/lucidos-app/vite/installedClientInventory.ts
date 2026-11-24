// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import fs from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';
// @ts-expect-error: same
import { createHash } from 'node:crypto';
import type { Plugin } from 'vite';

export const CLIENT_INVENTORY_FILE = 'workspace-client-assets.json';

export function installedClientInventory(defaultOutDir: string, publicDir: string, clientVersion: () => number): Plugin {
  return {
    name: 'lucidos-installed-client-inventory',
    apply: 'build',
    writeBundle(options, bundle) {
      const outDir = options.dir ?? defaultOutDir;
      const names = new Set(Object.keys(bundle));
      const walk = (relative: string) => {
        for (const entry of fs.readdirSync(resolve(publicDir, relative), { withFileTypes: true })) {
          if (entry.name.startsWith('.')) continue;
          const name = relative ? `${relative}/${entry.name}` : entry.name;
          if (entry.isDirectory()) walk(name);
          else if (entry.isFile()) names.add(name);
        }
      };
      walk('');
      const files: Record<string, string> = {};
      for (const name of [...names].sort()) {
        if (name === 'sw.js' || name === CLIENT_INVENTORY_FILE || name.endsWith('.map')) continue;
        files[name] = createHash('sha256').update(fs.readFileSync(resolve(outDir, name))).digest('hex');
      }
      if (!files['index.html']) throw new Error('Installed client inventory requires index.html');
      fs.writeFileSync(resolve(outDir, CLIENT_INVENTORY_FILE), JSON.stringify({ version: 1, client_version: clientVersion(), files }));
    },
  };
}
