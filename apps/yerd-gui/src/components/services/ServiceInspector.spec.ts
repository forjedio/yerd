import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  serviceOverrides: vi.fn(),
  setServiceOverride: vi.fn(),
  unsetServiceOverride: vi.fn(),
  createDatabase: vi.fn(),
  dropDatabase: vi.fn(),
  backupDatabase: vi.fn(),
  restoreDatabase: vi.fn(),
  pickSaveFile: vi.fn(),
  pickOpenFile: vi.fn(),
  createBucket: vi.fn(),
  dropBucket: vi.fn(),
  s3Credentials: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({ IpcError: class IpcError extends Error {}, ...client }));

import ServiceIcon from "./ServiceIcon.vue";
import ServiceInspector from "./ServiceInspector.vue";
import type { ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";
import { envText, reverbEnv, reverbPublic, s3Env, sqlEnv } from "@/lib/services";

const Pass = { template: "<div><slot /></div>" };
const MenuStub = {
  name: "MenuStub",
  props: ["open"],
  emits: ["update:open"],
  template: '<div data-testid="dropdown"><slot /></div>',
};
const ItemStub = {
  props: ["disabled"],
  emits: ["select"],
  template:
    '<div role="menuitem" :data-disabled="disabled ? \'\' : undefined" @click="$emit(\'select\')"><slot /></div>',
};

const mounted: VueWrapper[] = [];
const clipboardWriteText = vi.fn();

function svc(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "postgres",
    display_name: "PostgreSQL",
    installed_versions: ["18.4"],
    selected_version: "18.4",
    state: "running",
    pid: 56020,
    listen: "127.0.0.1:5432",
    port: 5432,
    enabled: true,
    supports_databases: true,
    supports_overrides: true,
    ...overrides,
  };
}

const redis = svc({
  service: "redis",
  display_name: "Redis (Valkey)",
  supports_databases: false,
  supports_overrides: false,
  port: 6379,
});

const reverb = svc({
  service: "reverb:codestash",
  type_id: "reverb",
  display_name: "Reverb",
  installed_versions: [],
  selected_version: null,
  supports_databases: false,
  supports_overrides: false,
  site: "codestash",
  port: 8081,
});

const s3 = svc({
  service: "versitygw",
  type_id: "versitygw",
  display_name: "S3 (versitygw)",
  installed_versions: ["1.7.0"],
  selected_version: "1.7.0",
  supports_databases: false,
  supports_overrides: false,
  port: 7070,
});

const S3_STATE = {
  buckets: { names: ["avatars", "codestash-uploads"], loading: false, error: null },
  stats: { avatars: { objects: 1284, bytes: 312_400_000 }, "codestash-uploads": { objects: 86, bytes: 48_100_000 } },
  credentials: { endpoint: "http://127.0.0.1:7070", access_key: "yerd_7Kq2XH9PmA" },
};

const SITES: SiteEntry[] = [
  { name: "codestash", document_root: "/srv/c", php: "8.4", secure: true, kind: "linked", is_laravel: true },
];

const REPORT = { tld: "test", http: { bound: 80 }, https: { bound: 443 }, port_redirect: false } as unknown as StatusReport;

async function mountInspector(props: Record<string, unknown> = {}) {
  const w = mount(ServiceInspector, {
    attachTo: document.body,
    props: {
      mode: "panel",
      shown: true,
      service: svc(),
      databases: { names: ["app", "app_test_1", "blog"], loading: false, error: null },
      report: REPORT,
      sites: SITES,
      tld: "test",
      busy: null,
      ...props,
    },
    global: {
      stubs: {
        teleport: true,
        DropdownMenu: MenuStub,
        DropdownMenuTrigger: Pass,
        DropdownMenuContent: Pass,
        DropdownMenuItem: ItemStub,
        DropdownMenuSeparator: { template: "<hr />" },
      },
    },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

type Inspector = Awaited<ReturnType<typeof mountInspector>>;

function actions(w: Inspector): string[] {
  return w
    .get('[data-testid="service-header"]')
    .findAll("button[data-action]")
    .map((b) => b.attributes("data-action")!);
}

function connectEnv(w: Inspector): string {
  return w.get('[data-testid="connect-env"]').text();
}

beforeEach(() => {
  for (const fn of Object.values(client)) fn.mockReset().mockResolvedValue(undefined);
  client.serviceOverrides.mockResolvedValue({});
  clipboardWriteText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("ServiceInspector header", () => {
  it.each([
    ["running", ["logs", "restart", "stop"], "Running · PID 56020"],
    ["stopped", ["logs", "start"], "Stopped"],
    ["failed", ["logs", "start", "stop"], "Failed"],
  ] as const)("shows the buttons that apply when %s", async (state, expected, status) => {
    const w = await mountInspector({ service: svc({ state, error: state === "failed" ? "exit 1" : null }) });
    expect(actions(w)).toEqual(expected);
    expect(w.get('[data-testid="service-status"]').text()).toBe(status);
    expect(w.find('[data-testid="service-error"]').exists()).toBe(state === "failed");
  });

  it("emits each header action", async () => {
    const w = await mountInspector();
    for (const action of ["logs", "restart", "stop"]) {
      await w.get(`[data-testid="service-header"] button[data-action="${action}"]`).trigger("click");
    }
    expect(w.emitted("logs")).toHaveLength(1);
    expect(w.emitted("restart")).toHaveLength(1);
    expect(w.emitted("stop")).toHaveLength(1);
  });

  it("offers Uninstall for an engine and Remove this instance for a per-site one", async () => {
    const w = await mountInspector();
    const uninstall = w.get('[data-action="uninstall"]');
    expect(uninstall.text()).toContain("Uninstall PostgreSQL 18.4…");
    await uninstall.trigger("click");
    expect(w.emitted("uninstall")).toHaveLength(1);

    await w.setProps({ service: reverb });
    await flushPromises();
    expect(w.find('[data-action="uninstall"]').exists()).toBe(false);
    await w.get('[data-action="remove"]').trigger("click");
    expect(w.emitted("remove")).toHaveLength(1);
  });

  it("disables the actions while one runs for this service", async () => {
    const w = await mountInspector({ busy: "restart:postgres" });
    const restart = w.get('button[data-action="restart"]');
    expect(restart.attributes("disabled")).toBeDefined();
    expect(restart.find(".animate-spin").exists()).toBe(true);
    expect(w.get('button[data-action="stop"]').attributes("disabled")).toBeDefined();
  });

  it("shows a close button only in the drawer", async () => {
    const w = await mountInspector();
    expect(w.find('[aria-label="Close service details"]').exists()).toBe(false);
    await w.setProps({ mode: "drawer" });
    await w.get('[aria-label="Close service details"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
  });
});

describe("ServiceInspector facts", () => {
  it("shows the address, version and autostart for an engine", async () => {
    const w = await mountInspector();
    const facts = w.get('[data-testid="service-facts"]');
    expect(facts.text()).toContain("127.0.0.1:5432");
    expect(w.get('[data-testid="fact-version"]').text()).toContain("18.4");
    await facts.findAll("button").find((b) => b.text() === "Edit port")!.trigger("click");
    await facts.findAll("button").find((b) => b.text() === "Change")!.trigger("click");
    expect(w.emitted("editPort")).toHaveLength(1);
    expect(w.emitted("changeVersion")).toHaveLength(1);

    await w.get('[aria-label="Start PostgreSQL with Yerd"]').trigger("click");
    expect(w.emitted("toggleAutostart")?.[0]).toEqual([false]);
  });

  it("shows the site with Open for a per-site instance, disabled when the site is gone", async () => {
    const w = await mountInspector({ service: reverb });
    const site = w.get('[data-testid="fact-site"]');
    expect(site.text()).toContain("codestash.test");
    expect(w.find('[data-testid="fact-version"]').exists()).toBe(false);
    await site.get("button").trigger("click");
    expect(w.emitted("openSite")?.[0]).toEqual([SITES[0]]);

    await w.setProps({ sites: [] });
    expect(w.get('[data-testid="fact-site"] button').attributes("disabled")).toBeDefined();
  });
});

describe("ServiceInspector body", () => {
  it("shows databases, connect and overrides for a SQL engine", async () => {
    const w = await mountInspector();
    expect(w.find('[data-testid="databases-card"]').exists()).toBe(true);
    expect(w.find('[data-testid="connect-card"]').exists()).toBe(true);
    expect(w.find('[data-testid="overrides-card"]').exists()).toBe(true);
    expect(connectEnv(w)).toContain("DB_DATABASE=app");
  });

  it("shows only Connect for Redis without override support", async () => {
    const w = await mountInspector({ service: redis, databases: { names: null, loading: false, error: null } });
    expect(w.find('[data-testid="databases-card"]').exists()).toBe(false);
    expect(w.find('[data-testid="overrides-card"]').exists()).toBe(false);
    expect(connectEnv(w)).toContain("REDIS_PORT=6379");
  });

  it("shows Reverb's two-column Connect block", async () => {
    const w = await mountInspector({ service: reverb, databases: { names: null, loading: false, error: null } });
    const text = connectEnv(w);
    expect(text).toContain("Server");
    expect(text).toContain("Browser (Echo) — via the site's /app proxy");
    expect(text).toContain("VITE_REVERB_HOST=codestash.test");
    expect(w.text()).toContain("Connect codestash.test");
  });

  it("points Connect at a database chosen from its row menu", async () => {
    const w = await mountInspector();
    await w.get('[data-db="blog"] [data-action="use"]').trigger("click");
    await flushPromises();
    expect(connectEnv(w)).toContain("DB_DATABASE=blog");
    expect((w.get('select[aria-label="Database"]').element as HTMLSelectElement).value).toBe("blog");
  });

  it("copies the exact .env text for a SQL engine and for Reverb", async () => {
    const w = await mountInspector();
    const copy = () => w.findAll("button").find((b) => b.text().trim() === "Copy for .env")!.trigger("click");
    await copy();
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenLastCalledWith(envText(sqlEnv("postgres", 5432, "app")));

    await w.setProps({ service: reverb, databases: { names: null, loading: false, error: null } });
    await flushPromises();
    await copy();
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenLastCalledWith(
      envText(reverbEnv(8081, reverbPublic(SITES[0]!, "codestash", REPORT))),
    );
  });

  it("resets the Connect database when the service changes", async () => {
    const w = await mountInspector();
    await w.get('[data-db="blog"] [data-action="use"]').trigger("click");
    await flushPromises();
    await w.setProps({
      service: svc({ service: "mysql", type_id: "mysql", display_name: "MySQL", port: 3306 }),
      databases: { names: ["shop", "blog"], loading: false, error: null },
    });
    await flushPromises();
    expect(connectEnv(w)).toContain("DB_DATABASE=shop");
  });
});

describe("ServiceInspector S3 storage", () => {
  async function mountS3(props: Record<string, unknown> = {}) {
    return mountInspector({
      service: s3,
      databases: { names: null, loading: false, error: null },
      s3: S3_STATE,
      ...props,
    });
  }

  it("shows Buckets, Connect and Credentials and no overrides", async () => {
    const w = await mountS3();
    expect(w.find('[data-testid="buckets-card"]').exists()).toBe(true);
    expect(w.find('[data-testid="connect-card"]').exists()).toBe(true);
    expect(w.find('[data-testid="credentials-card"]').exists()).toBe(true);
    expect(w.find('[data-testid="overrides-card"]').exists()).toBe(false);
    expect(w.find('[data-testid="databases-card"]').exists()).toBe(false);
    expect(connectEnv(w)).toBe(
      envText(s3Env("http://127.0.0.1:7070", "yerd_7Kq2XH9PmA", null, "avatars")).replace(/\n/g, ""),
    );
  });

  it("points Connect at a bucket chosen with Use in Connect", async () => {
    const w = await mountS3();
    await w.get('[data-bucket="codestash-uploads"] [data-action="use"]').trigger("click");
    await flushPromises();
    expect(connectEnv(w)).toContain("AWS_BUCKET=codestash-uploads");
    expect((w.get('select[aria-label="Bucket"]').element as HTMLSelectElement).value).toBe("codestash-uploads");
  });

  it("copies the .env block with the secret fetched for the clipboard", async () => {
    client.s3Credentials.mockResolvedValue({
      endpoint: "http://127.0.0.1:7070",
      access_key: "yerd_7Kq2XH9PmA",
      secret_key: "s3cr3t",
    });
    const w = await mountS3();
    await w.findAll("button").find((b) => b.text().trim() === "Copy for .env")!.trigger("click");
    await flushPromises();
    expect(client.s3Credentials).toHaveBeenCalledWith(true);
    expect(clipboardWriteText).toHaveBeenLastCalledWith(
      envText(s3Env("http://127.0.0.1:7070", "yerd_7Kq2XH9PmA", "s3cr3t", "avatars")),
    );
    expect(connectEnv(w)).not.toContain("s3cr3t");
  });

  it("forwards bucket events and Start from the stopped card", async () => {
    client.dropBucket.mockResolvedValue(3);
    const w = await mountS3();
    await w.get('[data-bucket="avatars"] [data-action="drop"]').trigger("click");
    await flushPromises();
    await w.findAll("button").find((b) => b.text().trim() === "Delete")!.trigger("click");
    await flushPromises();
    expect(w.emitted("bucketDropped")).toEqual([["avatars"]]);

    await w.setProps({ service: { ...s3, state: "stopped" }, s3: { ...S3_STATE, buckets: { names: null, loading: false, error: null }, stats: {} } });
    await flushPromises();
    expect(w.text()).toContain("S3 is stopped");
    expect(w.text()).toContain("Start S3 (versitygw) to pick a bucket.");
    await w.findAll("button").find((b) => b.text().trim() === "Start S3")!.trigger("click");
    expect(w.emitted("start")).toHaveLength(1);
  });

  it("uses the hard drive icon for S3", () => {
    const icon = mount(ServiceIcon, { props: { typeId: "versitygw" } });
    expect(icon.html()).toContain("lucide-hard-drive");
    icon.unmount();
  });
});
