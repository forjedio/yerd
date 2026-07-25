import { describe, expect, it } from "vitest";

import { certHealth, deriveTrayHealth, trayAlerts, trayServiceRows } from "./trayHealth";
import type { StatusReport } from "@/ipc/types";

function baseReport(over: Partial<StatusReport> = {}): StatusReport {
  return {
    daemon_pid: 1,
    uptime_secs: 10,
    daemon_rss_bytes: null,
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    dns_addr: "127.0.0.1:5353",
    ca: {
      path: "/tmp/ca.pem",
      fingerprint: "a".repeat(64),
      trusted_system: true,
      php_trusts_ca: true,
    },
    resolver_installed: true,
    default_php: "8.4",
    php: [
      {
        version: "8.4",
        installed_patch: "8.4.1",
        state: "running",
        pid: 2,
        listen: "/tmp/php.sock",
        rss_bytes: null,
        update_available: null,
      },
    ],
    sites: { parked: 0, linked: 1, secured: 1 },
    load_avg: null,
    daemon_version: "2.0.3",
    services: [],
    ...over,
  };
}

describe("deriveTrayHealth", () => {
  it("returns bad when report is missing", () => {
    expect(deriveTrayHealth(null)).toBe("bad");
  });

  it("returns ok for a healthy report", () => {
    expect(deriveTrayHealth(baseReport())).toBe("ok");
  });

  it("returns bad for web_unbound", () => {
    expect(deriveTrayHealth(baseReport({ web_unbound: { http: 8080, https: 8443 } }))).toBe(
      "bad",
    );
  });

  it("returns warn for untrusted CA", () => {
    expect(
      deriveTrayHealth(
        baseReport({
          ca: {
            path: "/tmp/ca.pem",
            fingerprint: "a".repeat(64),
            trusted_system: false,
            php_trusts_ca: true,
          },
        }),
      ),
    ).toBe("warn");
  });

  it("returns warn for enabled stopped service", () => {
    expect(
      deriveTrayHealth(
        baseReport({
          services: [
            {
              service: "redis",
              display_name: "Redis",
              installed_versions: ["7"],
              selected_version: "7",
              state: "stopped",
              pid: null,
              listen: null,
              port: 6379,
              enabled: true,
              supports_databases: false,
            },
          ],
        }),
      ),
    ).toBe("warn");
  });
});

describe("trayServiceRows", () => {
  it("includes only running or failed PHP pools", () => {
    const rows = trayServiceRows(baseReport());
    expect(rows).toHaveLength(1);
    expect(rows[0]?.id).toBe("php:8.4");
    expect(rows[0]?.label).toBe("PHP 8.4");
    expect(rows[0]?.kind).toBe("php_pool");
    expect(rows[0]?.canRestart).toBe(true);
    expect(rows[0]?.phpVersion).toBe("8.4");
  });

  it("returns empty when no PHP pool is running or failed and no managed services", () => {
    const rows = trayServiceRows(
      baseReport({
        php: [
          {
            version: "8.4",
            installed_patch: "8.4.1",
            state: "stopped",
            pid: null,
            listen: null,
            rss_bytes: null,
            update_available: null,
          },
        ],
        services: [
          {
            service: "redis",
            display_name: "Redis",
            installed_versions: [],
            selected_version: null,
            state: "stopped",
            pid: null,
            listen: null,
            port: 6379,
            enabled: false,
            supports_databases: false,
          },
        ],
      }),
    );
    expect(rows).toEqual([]);
  });

  it("includes installed managed services such as Postgres", () => {
    const rows = trayServiceRows(
      baseReport({
        services: [
          {
            service: "redis",
            display_name: "Redis",
            installed_versions: ["7"],
            selected_version: "7",
            state: "running",
            pid: 99,
            listen: "127.0.0.1:6379",
            port: 6379,
            enabled: true,
            supports_databases: false,
          },
          {
            service: "postgres",
            display_name: "PostgreSQL",
            installed_versions: ["17"],
            selected_version: "17",
            state: "running",
            pid: 100,
            listen: "127.0.0.1:5432",
            port: 5432,
            enabled: true,
            supports_databases: true,
          },
        ],
      }),
    );
    expect(rows).toHaveLength(3);
    expect(rows[0]?.id).toBe("php:8.4");
    const pg = rows.find((r) => r.id === "postgres");
    expect(pg?.label).toBe("PostgreSQL");
    expect(pg?.kind).toBe("managed");
    expect(pg?.canStop).toBe(true);
    expect(pg?.canRestart).toBe(true);
  });

  it("shows start controls for installed but stopped managed services", () => {
    const rows = trayServiceRows(
      baseReport({
        services: [
          {
            service: "postgres",
            display_name: "PostgreSQL",
            installed_versions: ["17"],
            selected_version: "17",
            state: "stopped",
            pid: null,
            listen: null,
            port: 5432,
            enabled: true,
            supports_databases: true,
          },
        ],
      }),
    );
    const pg = rows.find((r) => r.id === "postgres");
    expect(pg?.canStart).toBe(true);
    expect(pg?.health).toBe("warn");
  });

  it("allows restart on failed php pool", () => {
    const rows = trayServiceRows(
      baseReport({
        php: [
          {
            version: "8.3",
            installed_patch: "8.3.0",
            state: "failed",
            pid: null,
            listen: null,
            rss_bytes: null,
            update_available: null,
          },
        ],
      }),
    );
    const php = rows.find((r) => r.id === "php:8.3");
    expect(php?.canRestart).toBe(true);
    expect(php?.health).toBe("bad");
  });
});

describe("certHealth", () => {
  it("is ok when trusted", () => {
    expect(certHealth(baseReport())).toBe("ok");
  });

  it("is bad when not trusted", () => {
    expect(
      certHealth(
        baseReport({
          ca: {
            path: "/tmp/ca.pem",
            fingerprint: "a".repeat(64),
            trusted_system: false,
          },
        }),
      ),
    ).toBe("bad");
  });
});

describe("trayAlerts", () => {
  it("returns daemon-down when report is missing", () => {
    expect(trayAlerts(null)[0]?.id).toBe("daemon-down");
  });

  it("includes CA alert when untrusted", () => {
    const alerts = trayAlerts(
      baseReport({
        ca: {
          path: "/tmp/ca.pem",
          fingerprint: "a".repeat(64),
          trusted_system: false,
          php_trusts_ca: true,
        },
      }),
    );
    expect(alerts.some((a) => a.id === "ca-untrusted")).toBe(true);
  });
});
