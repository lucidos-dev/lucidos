import type * as DrawerOrderModule from '../components/drawer/drawerOrder';

type DrawerOrder = typeof DrawerOrderModule;

let drawerOrder: DrawerOrder | null = null;

export function provideDrawerOrder(module: DrawerOrder): void {
  drawerOrder = module;
}

export function orderedVisibleThreadIds(): string[] {
  return drawerOrder?.orderedVisibleThreadIds() ?? [];
}

export function activeViewKey(): string {
  return drawerOrder?.activeViewKey() ?? 'none';
}
