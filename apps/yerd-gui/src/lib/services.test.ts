import { describe, expect, it } from "vitest";

import {
  address,
  canStart,
  canStop,
  changeVersionNote,
  compareServiceVersions,
  connectKind,
  currentVersion,
  databasesFooter,
  defaultConnectDb,
  detailsCell,
  envText,
  fileName,
  groupDatabases,
  hasVersion,
  headerStatus,
  isInstalled,
  isValidDbName,
  logLineTone,
  meilisearchEnv,
  rustfsEnv,
  purgeCopy,
  redisEnv,
  reverbEnv,
  reverbPublic,
  serviceSections,
  siteDomain,
  sqlEnv,
  statusLine,
  testCopyBase,
  uninstallVersion,
  versionChoices,
  versionLabel,
} from "./services";
import type { ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";

function svc(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "mysql",
    display_name: "MySQL",
    installed_versions: ["9.7.1"],
    selected_version: "9.7.1",
    state: "running",
    pid: 42,
    listen: "127.0.0.1:3308",
    port: 3308,
    enabled: true,
    supports_databases: true,
    ...overrides,
  };
}

const postgres = svc({ service: "postgres", display_name: "PostgreSQL", port: 5432 });
const mariadb = svc({ service: "mariadb", display_name: "MariaDB", installed_versions: [], selected_version: null });
const redis = svc({ service: "redis", display_name: "Redis (Valkey)", supports_databases: false, port: 6379 });
const meili = svc({ service: "meilisearch", display_name: "Meilisearch", supports_databases: false });
const reverb = svc({
  service: "reverb:blog",
  type_id: "reverb",
  display_name: "Reverb",
  installed_versions: [],
  selected_version: null,
  supports_databases: false,
  site: "blog",
  port: 8081,
});
const unknown = svc({ service: "s3", display_name: "S3", supports_databases: false });

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    http: { bound: 80 },
    https: { bound: 443 },
    port_redirect: false,
    ...overrides,
  } as unknown as StatusReport;
}

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return { name: "blog", document_root: "/srv/blog", php: "8.4", secure: false, kind: "linked", ...overrides };
}

describe("service predicates", () => {
  it.each([
    ["installed engine", svc(), true, false, true],
    ["uninstalled engine", mariadb, false, false, true],
    ["per-site instance", reverb, true, false, true],
    ["stopped engine", svc({ state: "stopped" }), true, true, false],
    ["failed engine", svc({ state: "failed" }), true, true, true],
  ])("%s", (_, s, installed, startable, stoppable) => {
    expect(isInstalled(s)).toBe(installed);
    expect(canStart(s)).toBe(startable);
    expect(canStop(s)).toBe(stoppable);
  });

  it("labels versions and knows which rows carry one", () => {
    expect(versionLabel(svc({ selected_version: null, installed_versions: ["1", "2"] }))).toBe("2");
    expect(versionLabel(mariadb)).toBe("-");
    expect(hasVersion(svc())).toBe(true);
    expect(hasVersion(reverb)).toBe(false);
    expect(hasVersion(mariadb)).toBe(false);
  });
});

describe("serviceSections", () => {
  it("groups installed services in display order and keeps daemon order within a group", () => {
    const sections = serviceSections([reverb, redis, svc(), mariadb, postgres, meili, unknown]);
    expect(sections.map((s) => [s.group, s.label, s.items.map((i) => i.service)])).toEqual([
      ["databases", "Databases", ["mysql", "postgres"]],
      ["cache", "Cache & search", ["redis", "meilisearch", "s3"]],
      ["site", "Per-site", ["reverb:blog"]],
    ]);
  });

  it("omits empty groups", () => {
    expect(serviceSections([redis]).map((s) => s.group)).toEqual(["cache"]);
    expect(serviceSections([mariadb])).toEqual([]);
  });
});

describe("status copy", () => {
  it.each([
    [svc(), 16, "running · :3308 · 16 databases"],
    [svc(), 1, "running · :3308 · 1 database"],
    [svc(), null, "running · :3308"],
    [svc({ state: "stopped" }), 16, "stopped · :3308"],
    [svc({ state: "failed" }), null, "failed · :3308"],
    [reverb, null, "running · :8081"],
  ])("statusLine %#", (s, count, expected) => {
    expect(statusLine(s, count)).toBe(expected);
  });

  it.each([
    [svc({ pid: 56020 }), "Running · PID 56020"],
    [svc({ pid: null }), "Running"],
    [svc({ state: "stopped" }), "Stopped"],
    [svc({ state: "failed" }), "Failed"],
  ])("headerStatus %#", (s, expected) => {
    expect(headerStatus(s)).toBe(expected);
  });

  it.each([
    [svc(), 16, "16 databases"],
    [svc(), 1, "1 database"],
    [svc(), null, "—"],
    [svc({ state: "stopped" }), 4, "—"],
    [reverb, null, "WebSockets"],
    [meili, null, "—"],
  ])("detailsCell %#", (s, count, expected) => {
    expect(detailsCell(s, count)).toBe(expected);
  });

  it("builds the site domain and the address", () => {
    expect(siteDomain(reverb, "test")).toBe("blog.test");
    expect(siteDomain(svc(), "test")).toBe("");
    expect(siteDomain(reverb, "test", [site({ primary_domain: "corp.test" })])).toBe("corp.test");
    expect(siteDomain(reverb, "test", [site({ name: "other", primary_domain: "corp.test" })])).toBe("blog.test");
    expect(address(postgres)).toBe("127.0.0.1:5432");
  });
});

describe("test copies", () => {
  it.each([
    ["a_test_1", "a"],
    ["a_test", null],
    ["a_test_x", null],
    ["a_test_test_3", "a_test"],
    ["plain", null],
  ])("testCopyBase(%j) is %j", (name, expected) => {
    expect(testCopyBase(name)).toBe(expected);
  });

  const names = ["app", "app_test_1", "app_test_2", "app_test", "app_test_test_1", "orphan_test_4", "zeta"];

  it("folds copies into an existing base when hiding", () => {
    const { rows, counts } = groupDatabases(names, { hideCopies: true, filter: "" });
    expect(rows).toEqual([
      { name: "app", copies: 2 },
      { name: "app_test", copies: 1 },
      { name: "orphan_test_4", copies: 0 },
      { name: "zeta", copies: 0 },
    ]);
    expect(counts).toEqual({ shown: 4, total: 4, hiddenCopies: 3 });
  });

  it("lists everything when not hiding", () => {
    const { rows, counts } = groupDatabases(names, { hideCopies: false, filter: "" });
    expect(rows.map((r) => r.name)).toEqual(names);
    expect(rows.every((r) => r.copies === 0)).toBe(true);
    expect(counts).toEqual({ shown: 7, total: 7, hiddenCopies: 0 });
  });

  it("filters after grouping", () => {
    const { rows, counts } = groupDatabases(names, { hideCopies: true, filter: " APP " });
    expect(rows.map((r) => r.name)).toEqual(["app", "app_test"]);
    expect(counts).toEqual({ shown: 2, total: 4, hiddenCopies: 3 });
  });

  it.each([
    [{ shown: 12, total: 22, hiddenCopies: 214 }, true, "Showing 12 of 22 · 214 test copies hidden"],
    [{ shown: 1, total: 1, hiddenCopies: 1 }, true, "Showing 1 of 1 · 1 test copy hidden"],
    [{ shown: 3, total: 3, hiddenCopies: 0 }, true, "Showing 3 of 3"],
    [{ shown: 3, total: 3, hiddenCopies: 0 }, false, "Showing 3 of 3"],
  ])("databasesFooter %#", (counts, hide, expected) => {
    expect(databasesFooter(counts, hide)).toBe(expected);
  });

  it("starts Connect on the first database that is not a copy", () => {
    expect(defaultConnectDb(["app_test_1", "app", "b"])).toBe("app");
    expect(defaultConnectDb(["only_test_1"])).toBe("only_test_1");
    expect(defaultConnectDb([])).toBe("");
  });
});

describe(".env snippets", () => {
  it.each([
    ["mysql", 3306, "app", "DB_CONNECTION=mysql\nDB_HOST=127.0.0.1\nDB_PORT=3306\nDB_DATABASE=app\nDB_USERNAME=root\nDB_PASSWORD="],
    ["mariadb", 3307, "", "DB_CONNECTION=mariadb\nDB_HOST=127.0.0.1\nDB_PORT=3307\nDB_DATABASE=your_database\nDB_USERNAME=root\nDB_PASSWORD="],
    ["postgres", 5432, "main", "DB_CONNECTION=pgsql\nDB_HOST=127.0.0.1\nDB_PORT=5432\nDB_DATABASE=main\nDB_USERNAME=postgres\nDB_PASSWORD="],
    ["postgres", 5432, "  ", "DB_CONNECTION=pgsql\nDB_HOST=127.0.0.1\nDB_PORT=5432\nDB_DATABASE=your_database\nDB_USERNAME=postgres\nDB_PASSWORD="],
  ])("sqlEnv(%s)", (type, port, db, expected) => {
    const env = sqlEnv(type, port, db);
    expect(envText(env)).toBe(expected);
    expect(env.filter((l) => l.strong).map((l) => l.text)).toEqual([env[3]!.text]);
  });

  it("matches the Redis block", () => {
    expect(envText(redisEnv(6379))).toBe(
      "REDIS_CLIENT=phpredis\nREDIS_HOST=127.0.0.1\nREDIS_PASSWORD=null\nREDIS_PORT=6379\n\nCACHE_STORE=redis\nSESSION_DRIVER=redis\nQUEUE_CONNECTION=redis",
    );
  });

  it.each([
    ["blog", "SCOUT_DRIVER=meilisearch\nSCOUT_PREFIX=blog_\nMEILISEARCH_HOST=http://127.0.0.1:7700\nMEILISEARCH_KEY="],
    ["", "SCOUT_DRIVER=meilisearch\nSCOUT_PREFIX=your_app_\nMEILISEARCH_HOST=http://127.0.0.1:7700\nMEILISEARCH_KEY="],
  ])("meilisearchEnv(%j)", (prefix, expected) => {
    expect(envText(meilisearchEnv(7700, prefix))).toBe(expected);
  });

  it("matches the RustFS S3 block on the instance's port", () => {
    const env = rustfsEnv(9002);
    expect(envText(env)).toBe(
      "FILESYSTEM_DISK=s3\nAWS_ACCESS_KEY_ID=rustfsadmin\nAWS_SECRET_ACCESS_KEY=rustfsadmin\nAWS_DEFAULT_REGION=us-east-1\nAWS_BUCKET=your_bucket\nAWS_ENDPOINT=http://127.0.0.1:9002\nAWS_USE_PATH_STYLE_ENDPOINT=true",
    );
    expect(env.filter((l) => l.strong).map((l) => l.text)).toEqual(["AWS_ENDPOINT=http://127.0.0.1:9002"]);
  });

  function reverbText(host: string, port: number, scheme: string): string {
    return [
      "# Reverb server - Yerd supervises this on localhost",
      "REVERB_APP_ID=my-app-id",
      "REVERB_APP_KEY=my-app-key",
      "REVERB_APP_SECRET=my-app-secret",
      "REVERB_HOST=127.0.0.1",
      "REVERB_PORT=8081",
      "REVERB_SCHEME=http",
      "",
      `# Laravel Echo (browser) - reaches Reverb over ${host} via Yerd's /app proxy`,
      'VITE_REVERB_APP_KEY="${REVERB_APP_KEY}"',
      `VITE_REVERB_HOST=${host}`,
      `VITE_REVERB_PORT=${port}`,
      `VITE_REVERB_SCHEME=${scheme}`,
    ].join("\n");
  }

  it.each([
    ["secure", site({ secure: true }), report(), reverbText("blog.test", 443, "https")],
    ["plain", site(), report(), reverbText("blog.test", 80, "http")],
    ["redirected", site({ secure: true }), report({ https: { bound: 8443 }, port_redirect: true } as Partial<StatusReport>), reverbText("blog.test", 443, "https")],
    ["bound 8443", site({ secure: true }), report({ https: { bound: 8443 } } as Partial<StatusReport>), reverbText("blog.test", 8443, "https")],
    ["primary domain", site({ primary_domain: "www.blog.test" }), report(), reverbText("www.blog.test", 80, "http")],
    ["unknown site", null, null, reverbText("blog.test", 80, "http")],
  ])("reverbEnv %s", (_, s, r, expected) => {
    const env = reverbEnv(8081, reverbPublic(s, "blog", r));
    expect(envText(env)).toBe(expected);
    expect([...env.server, ...env.browser].filter((l) => l.strong).map((l) => l.text.split("=")[0])).toEqual([
      "REVERB_PORT",
      "VITE_REVERB_HOST",
    ]);
  });
});

describe("connectKind", () => {
  it.each([
    [svc(), "sql"],
    [postgres, "sql"],
    [redis, "redis"],
    [meili, "meilisearch"],
    [svc({ service: "rustfs", type_id: "rustfs", display_name: "RustFS", supports_databases: false }), "rustfs"],
    [reverb, "site"],
    [unknown, "none"],
  ])("%# is %s", (s, expected) => {
    expect(connectKind(s)).toBe(expected);
  });
});

describe("misc helpers", () => {
  it.each([
    ["/tmp/dump.sql", "dump.sql"],
    ["C:\\Users\\me\\dump.sql", "dump.sql"],
    ["dump.sql", "dump.sql"],
  ])("fileName(%j)", (path, expected) => {
    expect(fileName(path)).toBe(expected);
  });

  it.each([
    ['2024-01-01 ERROR:  database "x" does not exist', "error"],
    ["FATAL:  connection to client lost", "error"],
    ["2024-01-01T00:00:00Z 0 [ERROR] [MY-010] bad", "error"],
    ["LOG:  checkpoint starting: time", "plain"],
  ])("logLineTone(%j)", (line, expected) => {
    expect(logLineTone(line)).toBe(expected);
  });

  it.each([
    [236, "PostgreSQL stops and its binaries are removed. Your 236 databases stay on disk, so reinstalling picks them back up."],
    [1, "PostgreSQL stops and its binaries are removed. Your 1 database stays on disk, so reinstalling picks it back up."],
    [null, "PostgreSQL stops and its binaries are removed. Your stored data stays on disk, so reinstalling picks it back up."],
  ])("purgeCopy(%j)", (count, expected) => {
    expect(purgeCopy("PostgreSQL", count)).toBe(expected);
  });

  it.each([
    [236, "PostgreSQL stops, its binaries are removed and 236 databases are deleted."],
    [1, "PostgreSQL stops, its binaries are removed and 1 database is deleted."],
    [null, "PostgreSQL stops, its binaries are removed and its stored data is deleted."],
  ])("purgeCopy(%j, purge)", (count, expected) => {
    expect(purgeCopy("PostgreSQL", count, true)).toBe(expected);
  });

  it.each([
    ["my_app", true],
    ["_x", true],
    ["1abc", false],
    ["a-b", false],
    ["a".repeat(63), true],
    ["a".repeat(64), false],
    ["", false],
  ])("isValidDbName(%j)", (name, expected) => {
    expect(isValidDbName(name)).toBe(expected);
  });
});

describe("version changes", () => {
  it.each([
    ["9.10.0", "9.9.1", 1],
    ["8.4.0", "9.7.1", -1],
    ["17", "17.0", 0],
    ["18.4", "18.4", 0],
  ])("compareServiceVersions(%j, %j) sign", (a, b, sign) => {
    expect(Math.sign(compareServiceVersions(a, b))).toBe(sign);
  });

  it.each([
    [svc({ installed_versions: ["8.4.0", "9.7.1"], selected_version: "8.4.0" }), "8.4.0"],
    [svc({ installed_versions: ["9.10.0", "9.9.1"], selected_version: null }), "9.10.0"],
    [svc({ installed_versions: [], selected_version: null }), null],
  ])("currentVersion %#", (s, expected) => {
    expect(currentVersion(s)).toBe(expected);
  });

  it.each([
    [["8.4.0", "9.7.1", "9.8.0"], ["9.7.1"], "9.7.1", "9.8.0", ["8.4.0", "9.8.0"]],
    [["8.0.40", "8.4.9", "9.7.1"], ["9.7.1"], "9.7.1", "", ["8.0.40", "8.4.9"]],
    [["9.10.0", "9.9.0", "9.8.0"], ["9.8.0"], "9.8.0", "9.10.0", ["9.9.0", "9.10.0"]],
    [["1.0.0"], [], null, "1.0.0", ["1.0.0"]],
  ])("versionChoices %#", (available, installed, current, preselect, values) => {
    const choices = versionChoices(available, installed, current);
    expect(choices.preselect).toBe(preselect);
    expect(choices.options.map((o) => o.value)).toEqual(values);
  });

  it("flags downgrades", () => {
    const { options } = versionChoices(["8.4.9", "9.8.0"], ["9.7.1"], "9.7.1");
    expect(options.map((o) => o.downgrade)).toEqual([true, false]);
  });

  it.each([
    ["mysql", "9.7.1", "8.4.9", true, "can't move to an older version"],
    ["mariadb", "11.8.8", "11.4.2", true, "can't move to an older version"],
    ["postgres", "18.4", "17.6", true, "can't move to an older version"],
    ["redis", "9.1.0", "8.0.0", false, "Your stored data is kept"],
    ["mysql", "9.7.1", "9.8.0", false, "Your stored data is kept"],
    ["postgres", "17.2", "17.6", false, "Your stored data is kept"],
    ["postgres", "17.6", "18.4", false, "PostgreSQL 18 starts with an empty cluster"],
    ["meilisearch", "1.10.0", "1.11.0", false, "rebuild indexes"],
    ["mysql", "9.7.1", "", true, "can't move to an older version"],
    ["redis", "9.1.0", "", true, "Pick one to move to it"],
  ])("changeVersionNote(%j, %j -> %j)", (type, current, target, blocked, text) => {
    const note = changeVersionNote(type, type === "postgres" ? "PostgreSQL" : "Engine", current, target);
    expect(note.blocked).toBe(blocked);
    expect(note.text).toContain(text);
  });

  it("says a Postgres major change keeps the old cluster on disk", () => {
    expect(changeVersionNote("postgres", "PostgreSQL", "17.6", "18.4").text).toContain(
      "your PostgreSQL 17 databases stay on disk and come back if you switch back to a 17.x version",
    );
  });

  it.each([
    [svc({ installed_versions: ["8.4.0", "9.7.1"], selected_version: "8.4.0" }), "8.4.0"],
    [svc({ installed_versions: ["8.4.0", "9.7.1"], selected_version: "9.9.9" }), "9.7.1"],
    [svc({ installed_versions: ["9.10.0", "9.9.1"], selected_version: null }), "9.10.0"],
    [svc({ installed_versions: [], selected_version: null }), ""],
  ])("uninstallVersion %#", (s, expected) => {
    expect(uninstallVersion(s)).toBe(expected);
  });
});
