import { apiUrl, configure, SdkError } from './_fetch';
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
import { request } from './request';

export const lucidos = {
  configure,
  // Public because an app builds a `src` or an `href` onto the engine with it.
  // A relative URL resolves under the frame's own directory, and a
  // root-absolute `/api/v1/…` reads its first segment as a workspace name. Both
  // 404. Behind a gateway that directory also carries a frame capability (ADR
  // 0238), which reaches no engine route. A `fetch` of what this returns is
  // refused from a frame; `lucidos.request` is the call. See
  // `system-knowhow/js-sdk.md`.
  apiUrl,
  // The escape hatch for an endpoint no namespace covers (ADR 0231). Bridged,
  // and refused for a route the engine does not open to apps.
  request,
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

// The engine's answer for which routes an app may reach, and the matcher over
// it. Runtime values rather than an internal, because the host bridge enforces
// the SAME table this SDK checks, from this one copy. `parseAppId` travels
// with them, so the host names an app frame's app the way the frame does.
export { appMayCall, appReachableMethods, normalizeSuffix, pathMatchesPattern } from './appReach';
export { APP_REACHABLE_ROUTES } from './generated/app-reach';
export { parseAppId } from './scroll';

// Generated navigation contract (source of truth: the engine `navigate_ui`
// tool). Exposed as runtime values so the host app can cross-check them against
// its own renderable nav set — see `crates/lucidos-engine/src/llm/tools/misc.rs`.
export { NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from './generated/navigate-targets';
