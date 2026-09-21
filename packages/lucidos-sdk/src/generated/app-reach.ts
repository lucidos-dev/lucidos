// AUTO-GENERATED. Do not edit by hand.
// Regenerate: cargo test -p lucidos-engine --lib generate_app_reach_file -- --ignored
//
// Source of truth: ROUTE_REACH in crates/lucidos-engine/src/api/app_reach.rs
// (ADR 0231). Only app-reachable routes appear. A route absent from this
// table is refused, which is what makes the list safe to be short.

export interface AppReachableRoute {
  /** Route pattern, with `:param` and `*wildcard` segments. */
  path: string;
  methods: string[];
}

export const APP_REACHABLE_ROUTES: AppReachableRoute[] = [
  { path: '/app', methods: ['GET'] },
  { path: '/apps', methods: ['GET'] },
  { path: '/data', methods: ['GET'] },
  { path: '/data/*path', methods: ['GET', 'PUT', 'DELETE'] },
  { path: '/data/edit', methods: ['POST'] },
  { path: '/data/upload', methods: ['POST'] },
  { path: '/env-vars', methods: ['GET'] },
  { path: '/events/count', methods: ['GET'] },
  { path: '/events/emit', methods: ['POST'] },
  { path: '/events/query', methods: ['GET'] },
  { path: '/events/types', methods: ['GET'] },
  { path: '/health', methods: ['GET'] },
  { path: '/knowhow', methods: ['GET'] },
  { path: '/knowhow/read', methods: ['GET'] },
  { path: '/models', methods: ['GET'] },
  { path: '/notification', methods: ['GET'] },
  { path: '/notification/read', methods: ['POST'] },
  { path: '/notifications', methods: ['GET', 'POST'] },
  { path: '/notifications/before', methods: ['GET'] },
  { path: '/notifications/read-all', methods: ['POST'] },
  { path: '/oauth/:provider/access-token', methods: ['GET'] },
  { path: '/preferences', methods: ['GET', 'PUT'] },
  { path: '/proxy/:name', methods: ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'] },
  { path: '/proxy/:name/', methods: ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'] },
  { path: '/proxy/:name/*path', methods: ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'] },
  { path: '/threads/count', methods: ['GET'] },
  { path: '/threads/list', methods: ['GET'] },
  { path: '/trigger-groups', methods: ['GET'] },
  { path: '/triggers', methods: ['GET', 'POST', 'PUT', 'DELETE'] },
  { path: '/triggers/historical', methods: ['GET'] },
  { path: '/triggers/run', methods: ['POST'] },
  { path: '/ui/navigate', methods: ['POST'] },
];
