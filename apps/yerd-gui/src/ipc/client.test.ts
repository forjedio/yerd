import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke surface before importing the client.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
const homeDirMock = vi.fn();
vi.mock("@tauri-apps/api/path", () => ({
  homeDir: (...args: unknown[]) => homeDirMock(...args),
}));

import {
  addDomain,
  addRouteRule,
  availableNode,
  clearMails,
  clearNodeVersion,
  contributors,
  deleteMails,
  getMail,
  homeDirectory,
  IpcError,
  installNodeStreamed,
  installNodeWithProgress,
  listMails,
  listNode,
  listPhp,
  listResponseHeaders,
  listRoutes,
  listSites,
  nodeSite,
  nodeSites,
  releaseNotes,
  removeDomain,
  removeResponseHeader,
  removeRouteRule,
  resetDomains,
  setFrontController,
  setMailEnabled,
  setMailPort,
  setNodeVersion,
  setPhpDirectives,
  setPhpPoolSettings,
  setPhpVersionSettings,
  setPrimaryDomain,
  setResponseHeader,
  setSiteFavourite,
  setSymlinkProtection,
  setMcpEnabled,
  setXdebug,
  getNetworkSettings,
  setNetworkSettings,
  testNetwork,
  status,
  uninstallNode,
  unlink,
  updatePhp,
} from "./client";
import type { Site, StatusReport } from "./types";

beforeEach(() => invokeMock.mockReset());

const sampleSite: Site = {
  name: "blog",
  document_root: "/srv/blog",
  php: "8.5",
  secure: true,
  kind: "linked",
};

describe("client → command mapping", () => {
  it("listSites unwraps the sites array", async () => {
    invokeMock.mockResolvedValue({ type: "sites", sites: [sampleSite] });
    await expect(listSites()).resolves.toEqual([sampleSite]);
    expect(invokeMock).toHaveBeenCalledWith("list_sites", undefined);
  });

  it("status returns the inner report", async () => {
    const report = { daemon_pid: 7, tld: "test" } as unknown as StatusReport;
    invokeMock.mockResolvedValue({ type: "status", report });
    await expect(status()).resolves.toBe(report);
  });

  it("listPhp passes through installed/default/updates", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.4", "8.5"],
      default: "8.5",
      updates: [{ version: "8.5", installed: "8.5.6", latest: "8.5.7" }],
    });
    const r = await listPhp();
    expect(r.installed).toEqual(["8.4", "8.5"]);
    expect(r.updates?.[0].latest).toBe("8.5.7");
  });

  it("updatePhp(null) sends a null version (update-all)", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await updatePhp(null);
    expect(invokeMock).toHaveBeenCalledWith("update_php", { version: null });
  });

  it("listMails unwraps the mails array", async () => {
    invokeMock.mockResolvedValue({
      type: "mails",
      mails: [
        {
          id: "000001",
          from: "a@b.c",
          to: ["d@e.f"],
          subject: "Hi",
          date_epoch: 1700000000,
          site: "blog",
        },
      ],
    });
    const r = await listMails();
    expect(r).toHaveLength(1);
    expect(r[0].subject).toBe("Hi");
    expect(r[0].site).toBe("blog");
    expect(invokeMock).toHaveBeenCalledWith("list_mails", undefined);
  });

  it("getMail returns the inner detail", async () => {
    const mail = {
      id: "000001",
      from: "a@b.c",
      to: ["d@e.f"],
      subject: "Hi",
      date_epoch: 0,
      headers: [],
      html_body: "<p>Hi</p>",
      text_body: null,
    };
    invokeMock.mockResolvedValue({ type: "mail", mail });
    await expect(getMail("000001")).resolves.toEqual(mail);
    expect(invokeMock).toHaveBeenCalledWith("get_mail", { id: "000001" });
  });

  it("setMailPort sends the port; clearMails takes no args", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setMailPort(2525);
    expect(invokeMock).toHaveBeenCalledWith("set_mail_port", { port: 2525 });
    invokeMock.mockResolvedValue({ type: "ok" });
    await clearMails();
    expect(invokeMock).toHaveBeenCalledWith("clear_mails", undefined);
  });

  it("deleteMails sends the id list; setMailEnabled sends the flag", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await deleteMails(["000001", "000002"]);
    expect(invokeMock).toHaveBeenCalledWith("delete_mails", {
      ids: ["000001", "000002"],
    });
    invokeMock.mockResolvedValue({ type: "ok" });
    await setMailEnabled(true);
    expect(invokeMock).toHaveBeenCalledWith("set_mail_enabled", { enabled: true });
  });

  it("setSymlinkProtection sends the flag", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setSymlinkProtection(false);
    expect(invokeMock).toHaveBeenCalledWith("set_symlink_protection", { enabled: false });
  });

  it("setMcpEnabled sends the flag", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setMcpEnabled(true);
    expect(invokeMock).toHaveBeenCalledWith("set_mcp_enabled", { enabled: true });
  });

  it("network settings calls send camelCase args and unwrap the reply", async () => {
    const settings = { mode: "manual", http_proxy: "http://u:***@p:1", no_proxy: [] };
    invokeMock.mockResolvedValue({ type: "network_settings", settings });
    expect(await getNetworkSettings()).toEqual(settings);
    expect(invokeMock).toHaveBeenCalledWith("get_network_settings", undefined);
    await setNetworkSettings("manual", "http://u:***@p:1", "", [".corp.com"]);
    expect(invokeMock).toHaveBeenCalledWith("set_network_settings", {
      mode: "manual",
      httpProxy: "http://u:***@p:1",
      httpsProxy: "",
      noProxy: [".corp.com"],
    });
    const result = { ok: true, url: "https://files.yerd.app/latest.json", status: 200, source: "none" };
    invokeMock.mockResolvedValue({ type: "network_test", result });
    expect(await testNetwork()).toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("test_network", undefined);
  });

  it("setFrontController sends the site name and flag", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setFrontController("blog", true);
    expect(invokeMock).toHaveBeenCalledWith("set_front_controller", {
      name: "blog",
      enabled: true,
    });
  });

  it("addRouteRule sends the site, prefix and target", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await addRouteRule("portal", "/api", "api/index.php");
    expect(invokeMock).toHaveBeenCalledWith("add_route_rule", {
      site: "portal",
      prefix: "/api",
      target: "api/index.php",
    });
  });

  it("removeRouteRule sends the site and prefix", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await removeRouteRule("portal", "/api");
    expect(invokeMock).toHaveBeenCalledWith("remove_route_rule", {
      site: "portal",
      prefix: "/api",
    });
  });

  it("listRoutes returns the rules, and an empty list for an unexpected reply", async () => {
    invokeMock.mockResolvedValue({
      type: "routes",
      rules: [{ site: "portal", prefix: "/api", target: "api/index.php" }],
    });
    expect(await listRoutes()).toEqual([
      { site: "portal", prefix: "/api", target: "api/index.php" },
    ]);

    invokeMock.mockResolvedValue({ type: "ok" });
    expect(await listRoutes()).toEqual([]);
  });

  it("addRouteRule propagates a daemon rejection as an IpcError", async () => {
    invokeMock.mockResolvedValue({
      type: "error",
      code: "invalid_path",
      message: "target must be a relative path",
    });
    await expect(addRouteRule("portal", "/api", "../escape.php")).rejects.toBeInstanceOf(IpcError);
  });

  it("setResponseHeader sends the site, name and value", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setResponseHeader("api", "Access-Control-Allow-Origin", "https://app.test");
    expect(invokeMock).toHaveBeenCalledWith("set_response_header", {
      site: "api",
      name: "Access-Control-Allow-Origin",
      value: "https://app.test",
    });
  });

  it("removeResponseHeader sends the site and name", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await removeResponseHeader("api", "Access-Control-Allow-Origin");
    expect(invokeMock).toHaveBeenCalledWith("remove_response_header", {
      site: "api",
      name: "Access-Control-Allow-Origin",
    });
  });

  it("listResponseHeaders returns the headers, and an empty list for an unexpected reply", async () => {
    invokeMock.mockResolvedValue({
      type: "response_headers",
      headers: [{ site: "api", name: "X-Frame-Options", value: "DENY" }],
    });
    expect(await listResponseHeaders()).toEqual([
      { site: "api", name: "X-Frame-Options", value: "DENY" },
    ]);

    invokeMock.mockResolvedValue({ type: "ok" });
    expect(await listResponseHeaders()).toEqual([]);
  });

  it("setResponseHeader propagates a daemon rejection as an IpcError", async () => {
    invokeMock.mockResolvedValue({
      type: "error",
      code: "invalid_path",
      message: "invalid response header",
    });
    await expect(setResponseHeader("api", "Host", "x")).rejects.toBeInstanceOf(IpcError);
  });

  it("setFrontController rejects on a non-ok response", async () => {
    invokeMock.mockResolvedValue({ type: "error", code: "internal", message: "boom" });
    await expect(setFrontController("blog", false)).rejects.toThrow("boom");
  });

  it("setSiteFavourite sends site + favourite and rejects on error", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setSiteFavourite("blog", true);
    expect(invokeMock).toHaveBeenCalledWith("set_site_favourite", {
      site: "blog",
      favourite: true,
    });
    invokeMock.mockResolvedValue({ type: "error", code: "internal", message: "boom" });
    await expect(setSiteFavourite("blog", false)).rejects.toBeInstanceOf(IpcError);
  });

  it("addDomain / removeDomain / setPrimaryDomain send name + domain", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await addDomain("blog", "corp.test");
    expect(invokeMock).toHaveBeenCalledWith("add_domain", {
      name: "blog",
      domain: "corp.test",
    });
    invokeMock.mockResolvedValue({ type: "ok" });
    await removeDomain("blog", "*.blog.test");
    expect(invokeMock).toHaveBeenCalledWith("remove_domain", {
      name: "blog",
      domain: "*.blog.test",
    });
    invokeMock.mockResolvedValue({ type: "ok" });
    await setPrimaryDomain("blog", "corp.test");
    expect(invokeMock).toHaveBeenCalledWith("set_primary_domain", {
      name: "blog",
      domain: "corp.test",
    });
  });

  it("setPhpVersionSettings sends the version and settings map", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.3"],
      default: "8.3",
      version_settings: { "8.3": { memory_limit: "1G" } },
    });
    const r = await setPhpVersionSettings("8.3", { memory_limit: "1G" });
    expect(invokeMock).toHaveBeenCalledWith("set_php_version_settings", {
      version: "8.3",
      settings: { memory_limit: "1G" },
    });
    expect(r.version_settings?.["8.3"]?.memory_limit).toBe("1G");
  });

  it("setPhpDirectives sends the version and directives map", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.3"],
      default: "8.3",
      directives: { "8.3": { "xdebug.mode": "debug" } },
    });
    const r = await setPhpDirectives("8.3", { "xdebug.mode": "debug" });
    expect(invokeMock).toHaveBeenCalledWith("set_php_directives", {
      version: "8.3",
      directives: { "xdebug.mode": "debug" },
    });
    expect(r.directives?.["8.3"]?.["xdebug.mode"]).toBe("debug");
  });

  it("setPhpPoolSettings sends the version and settings map", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.4"],
      default: "8.4",
      pool: { "8.4": { max_children: "32" } },
    });
    const r = await setPhpPoolSettings("8.4", { max_children: "32" });
    expect(invokeMock).toHaveBeenCalledWith("set_php_pool_settings", {
      version: "8.4",
      settings: { max_children: "32" },
    });
    expect(r.pool?.["8.4"]?.["max_children"]).toBe("32");
  });

  it("setXdebug sends the version, toggle, modes and start", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.4"],
      default: "8.4",
      xdebug: { "8.4": { enabled: true, modes: ["debug", "profile"], start: "trigger" } },
      xdebug_versions: ["8.4"],
    });
    const r = await setXdebug("8.4", true, ["debug", "profile"], "trigger");
    expect(invokeMock).toHaveBeenCalledWith("set_xdebug", {
      version: "8.4",
      enabled: true,
      modes: ["debug", "profile"],
      start: "trigger",
    });
    expect(r.xdebug?.["8.4"]?.start).toBe("trigger");
    expect(r.xdebug_versions).toEqual(["8.4"]);
  });

  it("setXdebug sends null for omitted modes and start", async () => {
    invokeMock.mockResolvedValue({ type: "php_versions", installed: ["8.4"], default: "8.4" });
    await setXdebug("8.4", false);
    expect(invokeMock).toHaveBeenCalledWith("set_xdebug", {
      version: "8.4",
      enabled: false,
      modes: null,
      start: null,
    });
  });

  it("setXdebug sends a null enabled to keep the current state", async () => {
    invokeMock.mockResolvedValue({ type: "php_versions", installed: ["8.4"], default: "8.4" });
    await setXdebug("8.4", null, undefined, "trigger");
    expect(invokeMock).toHaveBeenCalledWith("set_xdebug", {
      version: "8.4",
      enabled: null,
      modes: null,
      start: "trigger",
    });
  });

  it("resetDomains sends just the name", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await resetDomains("blog");
    expect(invokeMock).toHaveBeenCalledWith("reset_domains", { name: "blog" });
  });
});

describe("error handling", () => {
  it("converts a daemon Response::Error into an IpcError with its code", async () => {
    invokeMock.mockResolvedValue({
      type: "error",
      code: "not_found",
      message: "no such site",
    });
    await expect(unlink("ghost")).rejects.toMatchObject({
      code: "not_found",
      message: "no such site",
    });
  });

});

describe("node commands", () => {
  const release = {
    version: "22.12.0",
    lts: true,
    codename: "Jod",
    default: true,
    uninstall_blocked: false,
  };

  it("listNode unwraps the installed releases", async () => {
    invokeMock.mockResolvedValue({ type: "node_versions", installed: [release] });
    await expect(listNode()).resolves.toEqual([release]);
    expect(invokeMock).toHaveBeenCalledWith("list_node", undefined);
  });

  it("installNodeWithProgress keeps polling through a failed status read", async () => {
    vi.useFakeTimers();
    try {
      invokeMock
        .mockResolvedValueOnce({ type: "job_started", job_id: "j1" })
        .mockRejectedValueOnce({ code: "unreachable", message: "daemon unreachable" })
        .mockResolvedValueOnce({
          type: "job_progress",
          state: "succeeded",
          phase: "",
          log: ["Installed Node 22.12.0"],
          next_cursor: 1,
          error: null,
        });
      const lines: string[] = [];
      const done = installNodeWithProgress("22", false, (l) => lines.push(...l));
      await vi.runAllTimersAsync();
      await expect(done).resolves.toBeUndefined();
      expect(lines).toEqual(["Installed Node 22.12.0"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("listNode lists the newest release first", async () => {
    const older = ["9.11.2", "24.22.3", "26.13.1", "24.3.0"].map((version) => ({
      ...release,
      version,
    }));
    invokeMock.mockResolvedValue({ type: "node_versions", installed: older });
    const listed = await listNode();
    expect(listed.map((r) => r.version)).toEqual(["26.13.1", "24.22.3", "24.3.0", "9.11.2"]);
  });

  it("availableNode unwraps the major lines", async () => {
    const line = { major: 22, latest: "22.12.0", update_available: false };
    invokeMock.mockResolvedValue({ type: "available_node", majors: [line] });
    await expect(availableNode()).resolves.toEqual([line]);
    expect(invokeMock).toHaveBeenCalledWith("available_node", undefined);
  });

  it("installNodeStreamed sends version and update and returns the job id", async () => {
    invokeMock.mockResolvedValue({ type: "job_started", job_id: "j9" });
    await expect(installNodeStreamed("22", true)).resolves.toBe("j9");
    expect(invokeMock).toHaveBeenCalledWith("install_node_streamed", {
      version: "22",
      update: true,
    });
  });

  it("installNodeWithProgress polls to the end and throws on failure", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "install_node_streamed") {
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      }
      return Promise.resolve({
        type: "job_progress",
        state: "failed",
        phase: "",
        log: ["Downloading"],
        next_cursor: 1,
        error: "checksum mismatch",
      });
    });
    const lines: string[] = [];
    await expect(
      installNodeWithProgress("24", false, (l) => lines.push(...l)),
    ).rejects.toMatchObject({ message: "checksum mismatch" });
    expect(lines).toEqual(["Downloading"]);
    expect(invokeMock).toHaveBeenCalledWith("job_status", { jobId: "j1", cursor: 0 });
  });

  it("uninstallNode sends the version and returns the refreshed list", async () => {
    invokeMock.mockResolvedValue({ type: "node_versions", installed: [] });
    await expect(uninstallNode("20.18.0")).resolves.toEqual([]);
    expect(invokeMock).toHaveBeenCalledWith("uninstall_node", { version: "20.18.0" });
  });

  it("setNodeVersion sends a null site for the global default", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setNodeVersion("22.12.0", null);
    expect(invokeMock).toHaveBeenCalledWith("set_node_version", {
      version: "22.12.0",
      site: null,
    });
    await setNodeVersion("20.18.0", "blog");
    expect(invokeMock).toHaveBeenLastCalledWith("set_node_version", {
      version: "20.18.0",
      site: "blog",
    });
  });

  it("clearNodeVersion sends the site", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await clearNodeVersion("blog");
    expect(invokeMock).toHaveBeenCalledWith("clear_node_version", { site: "blog" });
  });

  it("nodeSite passes the reply through with optional fields absent", async () => {
    invokeMock.mockResolvedValue({ type: "node_site", effective: "22.12.0", source: "default" });
    const r = await nodeSite("blog");
    expect(invokeMock).toHaveBeenCalledWith("node_site", { site: "blog" });
    expect(r.effective).toBe("22.12.0");
    expect(r.source).toBe("default");
    expect(r.preference).toBeUndefined();
  });

  it("nodeSites returns every entry and rejects an unexpected reply", async () => {
    invokeMock.mockResolvedValue({
      type: "node_sites",
      sites: [
        { site: "blog", effective: "22.12.0", source: "nvmrc" },
        { site: "shop", error: "Node 18 is not installed", missing: "18" },
      ],
    });
    const sites = await nodeSites();
    expect(invokeMock).toHaveBeenCalledWith("node_sites", undefined);
    expect(sites.map((s) => s.site)).toEqual(["blog", "shop"]);
    expect(sites[1].missing).toBe("18");
    invokeMock.mockResolvedValue({ type: "ok" });
    await expect(nodeSites()).rejects.toBeInstanceOf(IpcError);
  });

  it("releaseNotes passes current and earlier through and rejects an unexpected reply", async () => {
    const entry = {
      version: "2.0.4",
      tag: "v2.0.4",
      published_at: "2026-07-25T15:49:58Z",
      prerelease: false,
      summary: "Support xdebug",
      features: [{ title: "Support xdebug", author: "dev", url: "https://x.test/1" }],
      fixes: [],
      others: [],
    };
    invokeMock.mockResolvedValue({ type: "release_notes", earlier: [entry] });
    const r = await releaseNotes();
    expect(invokeMock).toHaveBeenCalledWith("release_notes", undefined);
    expect(r.current).toBeUndefined();
    expect(r.earlier[0].features[0].author).toBe("dev");
    invokeMock.mockResolvedValue({ type: "ok" });
    await expect(releaseNotes()).rejects.toBeInstanceOf(IpcError);
  });

  it("contributors returns the list and total and rejects an unexpected reply", async () => {
    invokeMock.mockResolvedValue({
      type: "contributors",
      contributors: [{ login: "totov", avatar_url: "https://a.test/1", commits: 12 }],
      total: 1,
    });
    const r = await contributors();
    expect(invokeMock).toHaveBeenCalledWith("contributors", undefined);
    expect(r.total).toBe(1);
    expect(r.contributors[0].login).toBe("totov");
    invokeMock.mockResolvedValue({ type: "ok" });
    await expect(contributors()).rejects.toBeInstanceOf(IpcError);
  });

  it("listNode rejects an unexpected response type", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await expect(listNode()).rejects.toBeInstanceOf(IpcError);
  });
});

describe("IpcError categorisation", () => {
  it("detects an unreachable daemon from the message", () => {
    expect(
      new IpcError("daemon unreachable: /run/user/1000/yerd/yerd.sock").unreachable,
    ).toBe(true);
    expect(new IpcError("daemon is not running").unreachable).toBe(true);
  });

  it("a typed daemon error is not 'unreachable'", () => {
    const e = new IpcError("no such site", "not_found");
    expect(e.code).toBe("not_found");
    expect(e.unreachable).toBe(false);
  });

  it("the explicit unreachable code sets the flag", () => {
    expect(new IpcError("x", "unreachable").unreachable).toBe(true);
  });
});

describe("homeDirectory", () => {
  it("returns the host's home directory", async () => {
    homeDirMock.mockResolvedValue("/Users/a");
    await expect(homeDirectory()).resolves.toBe("/Users/a");
  });

  it("resolves null when the path API fails", async () => {
    homeDirMock.mockRejectedValue(new Error("no path api"));
    await expect(homeDirectory()).resolves.toBeNull();
  });
});
