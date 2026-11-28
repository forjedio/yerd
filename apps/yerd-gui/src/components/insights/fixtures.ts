/**
 * Insights wire fixtures for the component and view specs, shaped like the
 * approved design's sample data. Test-only; nothing in the app imports it.
 */
import type {
  InsightsEvent,
  InsightsIssue,
  InsightsOverview,
  InsightsRequestDetail,
  InsightsRequestSummary,
} from "@/ipc/types";

const BASE = new Date(2026, 10, 27, 10, 5).getTime();
const MIN = 60_000;

/** A populated hour of data. */
export function overviewFixture(): InsightsOverview {
  const series = Array.from({ length: 61 }, (_, i) => {
    const wave = Math.sin(i / 4) * 8;
    const spike = i === 37 ? 180 : i === 36 ? 90 : 0;
    return {
      t_ms: BASE + i * MIN,
      count: 30,
      p50_ms: 62 + wave / 2 + (spike ? 6 : 0),
      p95_ms: 150 + wave * 3 + spike,
      errors: [16, 34, 37].includes(i) ? 2 : 0,
    };
  });
  return {
    range: "1h",
    kpis: {
      p50_ms: 68,
      p95_ms: 231,
      p50_delta_ms: -9,
      p95_spike_ms: 412,
      p95_spike_at_ms: BASE + 37 * MIN,
      requests: 1846,
      per_minute: 31,
      static_skipped: 412,
      failed_pct: 1.9,
      s5xx: 29,
      s4xx: 6,
      queries_avg: 14,
      queries_max: 212,
      queries_max_route: "GET /orders/{order}",
    },
    cold: 4,
    dropped_requests: 0,
    dropped_frames: 0,
    ext_seen: true,
    ext_available: true,
    series,
    breakdown: [
      { part: "app", share: 0.42, mean_ms: 29 },
      { part: "db", share: 0.38, mean_ms: 26 },
      { part: "http", share: 0.12, mean_ms: 8 },
      { part: "wait", share: 0.08, mean_ms: 5 },
    ],
    routes: [
      route("GET", "/orders/{order}", "orders.show", "OrderController@show", 412, 92, 318, 0.34, 52, ["n_plus_one"]),
      route("GET", "/dashboard", "dashboard", "DashboardController", 618, 71, 140, 0.29, 31, ["n_plus_one", "duplicate"]),
      route("POST", "/checkout", "checkout.store", "CheckoutController@store", 63, 180, 512, 0.14, 18, ["slow_query"]),
      route("GET", "/cart", "cart", "CartController@index", 301, 48, 96, 0.1, 6, ["large_read"]),
    ],
    queries: [
      { fingerprint: 'select * from "order_items" where "order_id" = ?', count: 19_776, total_ms: 38_420, mean_ms: 1.9, route_label: "GET /orders/{order}" },
      { fingerprint: 'select * from "users" where "users"."id" = ? limit 1', count: 14_832, total_ms: 12_310, mean_ms: 0.8, route_label: "GET /dashboard" },
      { fingerprint: 'select count(*) from "orders" where "status" = ? and "created_at" > ?', count: 63, total_ms: 11_466, mean_ms: 182, route_label: "POST /checkout" },
    ],
    issues: issuesFixture(),
  };
}

function route(
  method: string,
  label: string,
  name: string,
  action: string,
  requests: number,
  p50: number,
  p95: number,
  share: number,
  q: number,
  issue_kinds: InsightsIssue["kind"][],
): InsightsOverview["routes"][number] {
  return {
    method,
    label,
    kind: "template",
    name,
    action,
    requests,
    p50_ms: p50,
    p95_ms: p95,
    share,
    queries_per_request: q,
    issue_kinds,
  };
}

/** Five open issues and two dismissed ones. */
export function issuesFixture(): InsightsIssue[] {
  const last = { last_request_id: "b".repeat(32), last_request_ts_ms: BASE + 57 * MIN };
  return [
    {
      key: "0000000000000001",
      kind: "n_plus_one",
      sql: 'select * from "order_items" where "order_id" = ?',
      hint: "try ->with('items')",
      file: "/Users/me/Code/acme/app/Http/Controllers/OrderController.php",
      line: 42,
      route_method: "GET",
      route_label: "/orders/{order}",
      occurrences: 19_776,
      affected_requests: 412,
      impact_ms: 38_728,
      max_in_request: 48,
      ...last,
    },
    {
      key: "0000000000000002",
      kind: "n_plus_one",
      sql: 'select * from "users" where "users"."id" = ? limit 1',
      hint: "lazy-loaded relation in a Blade loop",
      file: "/Users/me/Code/acme/resources/views/dashboard/activity.blade.php",
      line: 17,
      route_method: "GET",
      route_label: "/dashboard",
      occurrences: 14_832,
      affected_requests: 618,
      impact_ms: 25_338,
      max_in_request: 24,
      ...last,
    },
    {
      key: "0000000000000003",
      kind: "duplicate",
      sql: 'select * from "settings" where "team_id" = 7',
      hint: "cache it per request",
      file: "/Users/me/Code/acme/app/Support/Settings.php",
      line: 31,
      route_method: "GET",
      route_label: "/dashboard",
      occurrences: 3708,
      affected_requests: 618,
      impact_ms: 3708,
      max_in_request: 6,
      ...last,
    },
    {
      key: "0000000000000004",
      kind: "slow_query",
      sql: 'select count(*) from "orders" where "status" = ? and "created_at" > ?',
      hint: "add an index on (status, created_at)",
      file: "/Users/me/Code/acme/app/Actions/Checkout/ReserveStock.php",
      line: 58,
      route_method: "POST",
      route_label: "/checkout",
      occurrences: 63,
      affected_requests: 63,
      impact_ms: 11_466,
      max_in_request: 1,
      ...last,
    },
    {
      key: "0000000000000005",
      kind: "large_read",
      sql: 'select * from "products"',
      hint: "paginate or select fewer rows",
      file: "/Users/me/Code/acme/app/Http/Controllers/CartController.php",
      line: 23,
      route_method: "GET",
      route_label: "/cart",
      occurrences: 301,
      affected_requests: 301,
      impact_ms: 5719,
      max_in_request: 1,
      ...last,
    },
    {
      key: "0000000000000006",
      kind: "duplicate",
      sql: 'select * from "teams" where "id" = ?',
      hint: "cache it per request",
      occurrences: 20,
      affected_requests: 10,
      impact_ms: 10,
      max_in_request: 2,
      dismissed: true,
    },
    {
      key: "0000000000000007",
      kind: "slow_query",
      sql: 'select * from "reports"',
      hint: "add an index",
      occurrences: 2,
      affected_requests: 2,
      impact_ms: 300,
      max_in_request: 1,
      dismissed: true,
    },
  ];
}

function summary(
  i: number,
  method: string,
  path: string,
  status: number,
  ms: number,
  queries: number,
  extra: Partial<InsightsRequestSummary> = {},
): InsightsRequestSummary {
  return {
    id: String(i).padStart(32, "a"),
    ts_ms: BASE + 58 * MIN - i * 13_000,
    method,
    path,
    route_label: path,
    status,
    duration_ms: ms,
    bytes: 48_000,
    content_type: "text/html; charset=UTF-8",
    traced: true,
    queries,
    dumps: 0,
    logs: 0,
    ...extra,
  };
}

/** The Requests tab's list. */
export function requestsFixture(): InsightsRequestSummary[] {
  return [
    summary(1, "GET", "/dashboard", 200, 92, 31, { dumps: 1 }),
    summary(2, "GET", "/orders/1042", 200, 318, 52, { dumps: 2, id: "b".repeat(32), route_label: "/orders/{order}" }),
    summary(3, "POST", "/checkout", 500, 512, 18, { logs: 1 }),
    summary(4, "GET", "/cart", 200, 71, 6),
    summary(5, "GET", "/", 200, 604, 0, { cold: true }),
    summary(6, "GET", "/orders/1038", 200, 276, 44),
    summary(7, "PUT", "/cart/items/88", 422, 48, 9),
    summary(8, "GET", "/dashboard", 200, 85, 31),
  ];
}

function ev(idx: number, category: InsightsEvent["category"], tMs: number, durMs: number | null, payload: Record<string, unknown>): InsightsEvent {
  return { idx, category, t_us: Math.round(tMs * 1000), dur_us: durMs == null ? undefined : Math.round(durMs * 1000), payload };
}

/** The drill-in of `/orders/1042`. */
export function detailFixture(): InsightsRequestDetail {
  const s = requestsFixture()[1];
  if (!s) throw new Error("fixture");
  const nPlusOne = Array.from({ length: 48 }, (_, i) =>
    ev(10 + i, "query", 21 + i * 3.4, 3.4, { sql: `select * from order_items where order_id = ${1000 + i}`, file: "app/Http/Controllers/OrderController.php", line: 42 }),
  );
  const events = [
    ev(0, "cache", 3.2, 0.1, { event: "hit", key: "team:7:settings" }),
    ev(1, "query", 8.9, 9.6, { sql: 'select * from "orders" where "id" = 1042' }),
    ev(2, "dump", 19.4, null, { value_text: 'App\\Models\\Order {#1042 status: "paid" …}' }),
    ...nPlusOne,
    ev(60, "http", 186.3, 18.2, { method: "GET", url: "api.stripe.com/v1/charges/ch_3P…", status: 200 }),
    ev(61, "query", 205, 1.1, { sql: 'select * from "settings" where "team_id" = 7' }),
    ev(62, "log", 241.7, null, { level: "info", message: 'Order viewed {"order":1042}' }),
    ev(63, "request", 318, null, { method: "GET", uri: "/orders/1042", status: 200 }),
  ];
  return {
    summary: {
      ...s,
      route_name: "orders.show",
      route_action: "OrderController@show",
      bytes: 49_357,
      app_ms: 88,
      db_ms: 201,
      http_ms: 18,
      wait_ms: 7,
    },
    events,
    issues: [
      {
        kind: "n_plus_one",
        key: "0000000000000001",
        event_indices: nPlusOne.map((e) => e.idx),
        count: 48,
        time_ms: 164,
        file: "/Users/me/Code/acme/app/Http/Controllers/OrderController.php",
        line: 42,
        hint: "eager-load with ->with('items')",
      },
    ],
  };
}
