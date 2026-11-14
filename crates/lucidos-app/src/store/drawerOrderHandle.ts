/** The entry chunk's way into `components/drawer/drawerOrder.ts`, which ships
 *  in the shell chunk with the family graph.
 *
 *  Nothing the entry chunk reaches may import the family graph, or the whole
 *  graph rides in it. `startShell` provides the real module before `<App/>`
 *  first renders. Until then no drawer is on screen, so the order is empty.
 *  See *drawer order handle* in `docs/glossary.md`. */
import type * as DrawerOrderModule from '../components/drawer/drawerOrder';

type DrawerOrder = typeof DrawerOrderModule;

let drawerOrder: DrawerOrder | null = null;

/** Called once, by `startShell`. */
export function provideDrawerOrder(module: DrawerOrder): void {
  drawerOrder = module;
}

export function orderedVisibleThreadIds(): string[] {
  return drawerOrder?.orderedVisibleThreadIds() ?? [];
}

/** Before the shell, a key no drawer view matches. */
export function activeViewKey(): string {
  return drawerOrder?.activeViewKey() ?? 'none';
}
