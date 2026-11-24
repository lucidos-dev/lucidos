import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs exist in the build test runner
import fs from 'node:fs';
// @ts-expect-error: same
import os from 'node:os';
// @ts-expect-error: same
import { join } from 'node:path';
// @ts-expect-error: same
import { createHash } from 'node:crypto';
import { CLIENT_INVENTORY_FILE, installedClientInventory } from '../../vite/installedClientInventory';

describe('installed client inventory', () => {
  it('hashes final emitted bytes and public assets, excluding workers, maps and stale files', () => {
    const dir = fs.mkdtempSync(join(os.tmpdir(), 'client-inventory-'));
    try {
      const out = join(dir, 'dist');
      const pub = join(dir, 'public');
      fs.mkdirSync(out);
      fs.mkdirSync(pub);
      for (const [name, bytes] of Object.entries({ 'index.html': '<head></head>', 'app.js': 'stamped build', 'shell.js': 'grouped dialogs and previews', 'app.js.map': '{}', 'stale.js': 'old', 'sw.js': 'worker', 'icon.png': 'icon' })) {
        fs.writeFileSync(join(out, name), bytes);
      }
      fs.writeFileSync(join(pub, 'sw.js'), 'worker');
      fs.writeFileSync(join(pub, 'icon.png'), 'icon');
      const hook = installedClientInventory(out, pub, () => 7).writeBundle as (options: {dir: string}, bundle: object) => void;
      hook({ dir: out }, { 'index.html': {}, 'app.js': {}, 'shell.js': { type: 'chunk', moduleIds: ['src/App.tsx', 'src/components/shared/ConfirmAndPromptDialogs.tsx'], code: 'before final rewrite' }, 'app.js.map': {} });
      const inventory = JSON.parse(fs.readFileSync(join(out, CLIENT_INVENTORY_FILE), 'utf8'));
      expect(inventory.version).toBe(1);
      expect(inventory.client_version).toBe(7);
      expect(Object.keys(inventory.files)).toEqual(['app.js', 'icon.png', 'index.html', 'shell.js']);
      expect(inventory.files['shell.js']).toBe(createHash('sha256').update('grouped dialogs and previews').digest('hex'));
      expect(inventory.files['app.js']).toBe(createHash('sha256').update('stamped build').digest('hex'));
    } finally {
      fs.rmSync(dir, { recursive: true });
    }
  });
});
