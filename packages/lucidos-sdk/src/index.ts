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

// Generated navigation contract (source of truth: the engine `navigate_ui`
// tool). Exposed as runtime values so the host app can cross-check them against
// its own renderable nav set — see `crates/lucidos-engine/src/llm/tools/misc.rs`.
export { NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from './generated/navigate-targets';
