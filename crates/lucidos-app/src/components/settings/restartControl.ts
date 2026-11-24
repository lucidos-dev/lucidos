export type RestartControlHome = 'overview' | 'debugging';

export function restartControlHome(packaged: boolean): RestartControlHome {
  return packaged ? 'debugging' : 'overview';
}
