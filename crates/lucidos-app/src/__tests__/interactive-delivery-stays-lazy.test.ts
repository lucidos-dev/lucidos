import { expect, it } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

for (const root of ['main.tsx', 'App.tsx']) {
  const graph = staticGraph(srcPath(root));
  for (const module of ['store/actions/entityReferences.ts', 'store/actions/thread-sync.ts', 'store/actions/interactive-delivery.ts', 'store/interactiveDeliveryDevice.ts', 'store/interactiveDelivery.ts', 'store/interactiveDeliveryRecords.ts']) {
    it(`${root} leaves ${module} outside first paint`, () => {
      const file = srcPath(module);
      expect(graph.has(file), graph.has(file) ? chainTo(graph, file) : '').toBe(false);
    });
  }
}


it('installs UI effects with the shell instead of blocking startup reads', () => {
  const graph = staticGraph(srcPath('main.tsx'));
  const file = srcPath('store/effects.ts');
  expect(graph.has(file), graph.has(file) ? chainTo(graph, file) : '').toBe(false);
  expect(staticGraph(srcPath('App.tsx')).has(file)).toBe(true);
});


for (const module of ['store/actions/chat.ts', 'store/actions/repositories.ts', 'store/actions/navigation-request.ts', 'store/actions/app-bridge.ts', 'store/backgroundChecks.ts', 'store/actions/app-update.ts', 'store/actions/engine-update.ts', 'store/actions/slowness.ts', 'store/actions/cross-workspace.ts']) {
  it(`starts initial reads without ${module}`, () => {
    const graph = staticGraph(srcPath('main.tsx'));
    const file = srcPath(module);
    expect(graph.has(file), graph.has(file) ? chainTo(graph, file) : '').toBe(false);
  });
}
