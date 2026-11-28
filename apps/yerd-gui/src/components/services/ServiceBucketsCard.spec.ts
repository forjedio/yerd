import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  createBucket: vi.fn(),
  dropBucket: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {
    code: string;
    constructor(message: string, code = "internal") {
      super(message);
      this.code = code;
    }
  },
  ...client,
}));

import ServiceBucketsCard from "./ServiceBucketsCard.vue";
import { useToast } from "@/composables/useToast";
import { IpcError } from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";

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

function s3(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "versitygw",
    type_id: "versitygw",
    display_name: "S3 (versitygw)",
    installed_versions: ["1.7.0"],
    selected_version: "1.7.0",
    state: "running",
    pid: 1,
    listen: "127.0.0.1:7070",
    port: 7070,
    enabled: true,
    supports_databases: false,
    ...overrides,
  };
}

const NAMES = ["avatars", "backups", "invoices", "tpfs-assets"];
const STATS = {
  avatars: { objects: 1284, bytes: 312_400_000 },
  backups: { objects: 12, bytes: 2_400_000_000 },
  invoices: { objects: 0, bytes: 0 },
  "tpfs-assets": { objects: 241, bytes: 96_700_000 },
};

async function mountCard(props: Record<string, unknown> = {}) {
  const w = mount(ServiceBucketsCard, {
    attachTo: document.body,
    props: {
      service: s3(),
      names: NAMES,
      loading: false,
      error: null,
      stats: STATS,
      endpoint: "http://127.0.0.1:7070",
      connectBucket: "avatars",
      ...props,
    },
    global: {
      stubs: {
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

type Card = Awaited<ReturnType<typeof mountCard>>;

function rows(w: Card): string[] {
  return w.findAll("[data-bucket]").map((r) => r.attributes("data-bucket")!);
}

function button(w: Card, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

async function menu(w: Card, bucket: string, action: string) {
  await w.get(`[data-bucket="${bucket}"] [data-action="${action}"]`).trigger("click");
  await flushPromises();
}

function toasts() {
  return useToast().toasts.value.map((t) => [t.title, t.detail]);
}

beforeEach(() => {
  for (const fn of Object.values(client)) fn.mockReset().mockResolvedValue(undefined);
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("ServiceBucketsCard", () => {
  it("lists buckets with object counts, sizes and footer totals", async () => {
    const w = await mountCard();
    expect(rows(w)).toEqual(NAMES);
    expect(w.get('[data-testid="buckets-count"]').text()).toBe("4");
    expect(w.get('[data-bucket="avatars"] [data-testid="bucket-objects"]').text()).toBe("1,284 objects");
    expect(w.get('[data-bucket="avatars"] [data-testid="bucket-size"]').text()).toBe("312.4 MB");
    expect(w.get('[data-bucket="invoices"] [data-testid="bucket-objects"]').text()).toBe("empty");
    expect(w.get('[data-bucket="invoices"] [data-testid="bucket-size"]').text()).toBe("—");
    expect(w.get('[data-testid="buckets-footer"]').text()).toBe("4 buckets · 1,537 objects · 2.8 GB");
  });

  it("shows pending stats and hides the footer until every stat is in", async () => {
    const w = await mountCard({ stats: { ...STATS, backups: "loading" } });
    expect(w.get('[data-bucket="backups"] [data-testid="bucket-objects"]').text()).toBe("…");
    expect(w.find('[data-testid="buckets-footer"]').exists()).toBe(false);
  });

  it("filters the rows", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Filter buckets"]').setValue("BACK");
    expect(rows(w)).toEqual(["backups"]);
  });

  it("disables Create and explains an invalid name", async () => {
    const w = await mountCard();
    await button(w, "New bucket").trigger("click");
    const input = w.get('[aria-label="New bucket name"]');
    expect(w.text()).toContain("3–63 characters: lowercase letters, digits, hyphens and dots.");
    await input.setValue("User_Uploads");
    expect(w.get('[data-testid="bucket-name-error"]').text()).toBe(
      "Use lowercase letters, digits, hyphens and dots only.",
    );
    expect(input.attributes("aria-invalid")).toBe("true");
    expect(button(w, "Create").attributes("disabled")).toBeDefined();
    await input.setValue("user-uploads");
    expect(w.find('[data-testid="bucket-name-error"]').exists()).toBe(false);
    expect(button(w, "Create").attributes("disabled")).toBeUndefined();
  });

  it("creates a bucket, toasts its URL and emits changed", async () => {
    const w = await mountCard();
    await button(w, "New bucket").trigger("click");
    await w.get('[aria-label="New bucket name"]').setValue("user-uploads");
    await button(w, "Create").trigger("click");
    await flushPromises();
    expect(client.createBucket).toHaveBeenCalledWith("user-uploads");
    expect(toasts()).toContainEqual(["Created bucket user-uploads", "Ready at http://127.0.0.1:7070/user-uploads"]);
    expect(w.emitted("changed")).toHaveLength(1);
    expect(w.find('[aria-label="New bucket name"]').exists()).toBe(false);
  });

  it("explains a duplicate bucket name", async () => {
    client.createBucket.mockRejectedValue(new IpcError("a bucket called avatars already exists", "already_exists"));
    const w = await mountCard();
    await button(w, "New bucket").trigger("click");
    await w.get('[aria-label="New bucket name"]').setValue("avatars");
    await button(w, "Create").trigger("click");
    await flushPromises();
    expect(toasts()).toContainEqual(["Couldn't create bucket", "A bucket called avatars already exists"]);
    expect(w.emitted("changed")).toBeUndefined();
  });

  it("passes other create failures through", async () => {
    client.createBucket.mockRejectedValue(new IpcError("bucket name must not start with xn--", "invalid_path"));
    const w = await mountCard();
    await button(w, "New bucket").trigger("click");
    await w.get('[aria-label="New bucket name"]').setValue("xn--abc");
    await button(w, "Create").trigger("click");
    await flushPromises();
    expect(toasts()).toContainEqual(["Couldn't create bucket", "bucket name must not start with xn--"]);
  });

  it("confirms a delete in place with the object count, then drops and emits", async () => {
    client.dropBucket.mockResolvedValue(241);
    const w = await mountCard();
    await menu(w, "tpfs-assets", "drop");
    const confirm = w.get('[data-testid="confirm-drop"]');
    expect(confirm.text()).toContain("Delete tpfs-assets and its 241 objects? This can't be undone.");
    expect(document.activeElement?.textContent?.trim()).toBe("Cancel");
    await button(w, "Delete").trigger("click");
    await flushPromises();
    expect(client.dropBucket).toHaveBeenCalledWith("tpfs-assets");
    expect(toasts()).toContainEqual(["Deleted bucket tpfs-assets", "241 objects removed"]);
    expect(w.emitted("dropped")).toEqual([["tpfs-assets"]]);
    expect(w.find('[data-testid="confirm-drop"]').exists()).toBe(false);
  });

  it("confirms an empty bucket without a count", async () => {
    const w = await mountCard();
    await menu(w, "invoices", "drop");
    expect(w.get('[data-testid="confirm-drop"]').text()).toContain("Delete invoices? This can't be undone.");
    await button(w, "Cancel").trigger("click");
    expect(w.find('[data-testid="confirm-drop"]').exists()).toBe(false);
  });

  it("toasts a failed delete and refreshes the partly emptied bucket", async () => {
    client.dropBucket.mockRejectedValue(new IpcError("BucketNotEmpty"));
    const w = await mountCard();
    await menu(w, "backups", "drop");
    await button(w, "Delete").trigger("click");
    await flushPromises();
    expect(toasts()).toContainEqual(["Couldn't delete bucket", "BucketNotEmpty"]);
    expect(w.emitted("dropped")).toBeUndefined();
    expect(w.emitted("changed")).toEqual([[]]);
  });

  it("emits Use in Connect", async () => {
    const w = await mountCard();
    await menu(w, "backups", "use");
    expect(w.emitted("useInConnect")).toEqual([["backups"]]);
  });

  it("shows the empty state with its own New bucket", async () => {
    const w = await mountCard({ names: [], stats: {} });
    const empty = w.get('[data-testid="buckets-empty"]');
    expect(empty.text()).toContain("No buckets yet");
    expect(empty.text()).toContain("Create a bucket, then point your app's AWS_BUCKET at it from the Connect card.");
    expect(w.find('[data-testid="new-bucket"]').exists()).toBe(false);
    await button(w, "New bucket").trigger("click");
    expect(w.find('[aria-label="New bucket name"]').exists()).toBe(true);
  });

  it("shows the stopped state and emits start", async () => {
    const w = await mountCard({ service: s3({ state: "stopped" }), names: null, stats: {} });
    const stopped = w.get('[data-testid="buckets-stopped"]');
    expect(stopped.text()).toContain("S3 is stopped");
    expect(stopped.text()).toContain(
      "Start S3 (versitygw) to list and manage buckets. Your buckets are safe on disk.",
    );
    expect(w.find('[data-testid="new-bucket"]').exists()).toBe(false);
    await button(w, "Start S3").trigger("click");
    expect(w.emitted("start")).toHaveLength(1);
  });

  it("shows a spinner while the first list loads and the error when it fails", async () => {
    const loading = await mountCard({ names: null, loading: true, stats: {} });
    expect(loading.find('[data-testid="buckets-list"]').exists()).toBe(false);
    expect(loading.html()).toContain("animate-spin");
    const failed = await mountCard({ names: null, error: "start S3 (versitygw) first", stats: {} });
    expect(failed.get('[data-testid="buckets-error"]').text()).toBe("start S3 (versitygw) first");
  });
});
