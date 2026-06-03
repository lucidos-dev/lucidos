import { configure, SdkError } from './_fetch';
import { data } from './data';
import { events } from './events';
import { triggers } from './triggers';
import { preferences } from './preferences';
import { notifications } from './notifications';
import { apps } from './apps';
import { threads } from './threads';
import { ui } from './ui';
import { sse } from './sse';
import { utils } from './utils';
import { capture } from './capture';
import { proxy } from './proxy';
import { oauth } from './oauth';

export const lucidos = {
  configure,
  data,
  events,
  triggers,
  preferences,
  notifications,
  apps,
  threads,
  ui,
  sse,
  utils,
  proxy,
  oauth,
  _capture: capture,
};

export { SdkError };
export type * from './types';
