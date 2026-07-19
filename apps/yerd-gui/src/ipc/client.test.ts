import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke surface before importing the client.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
  addDomain,
  clearMails,
  deleteMails,
  getMail,
  IpcError,
  listMails,
  listPhp,
  listSites,
  removeDomain,
  resetDomains,
  setFrontController,
  setMailEnabled,
  setMailPort,
  setPhpDirectives,
  setPhpVersionSettings,
  setPrimaryDomain,
  setSymlinkProtection,
  setMcpEnabled,
  status,
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
        },
      ],
    });
    const r = await listMails();
    expect(r).toHaveLength(1);
    expect(r[0].subject).toBe("Hi");
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

  it("setFrontController sends the site name and flag", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await setFrontController("blog", true);
    expect(invokeMock).toHaveBeenCalledWith("set_front_controller", {
      name: "blog",
      enabled: true,
    });
  });

  it("setFrontController rejects on a non-ok response", async () => {
    invokeMock.mockResolvedValue({ type: "error", code: "internal", message: "boom" });
    await expect(setFrontController("blog", false)).rejects.toThrow("boom");
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
