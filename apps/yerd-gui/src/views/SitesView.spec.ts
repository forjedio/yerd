import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import SitesView from "./SitesView.vue";
import SiteCard from "@/components/SiteCard.vue";
import { resetResourceCache } from "@/composables/useResource";
import type { SiteEntry } from "@/ipc/types";

function wpSite(name: string): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.3",
    secure: false,
    kind: "linked",
    is_wordpress: true,
    wp_auto_login: true,
  };
}

/** Deferred promise, so a test can control exactly when a given site's
 *  `wordpress_admin_users` call resolves. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function usersEnvelope(logins: string[]) {
  return {
    type: "wordpress_admin_users",
    users: logins.map((login) => ({ login, display_name: login })),
  };
}

async function mountSites() {
  const wrapper = mount(SitesView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  await flushPromises();
  return wrapper;
}

async function openEditFor(wrapper: Awaited<ReturnType<typeof mountSites>>, site: SiteEntry) {
  const cards = wrapper.findAllComponents(SiteCard);
  const card = cards.find((c) => c.props("site").name === site.name);
  if (!card) throw new Error(`no SiteCard rendered for ${site.name}`);
  await card.vm.$emit("edit", site);
  await flushPromises();
}

describe("SitesView WordPress auto-login edit dialog", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
  });

  it("loads the admin-user picker for the site the dialog was opened for", async () => {
    const alpha = wpSite("alpha");
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_sites") return Promise.resolve({ type: "sites", sites: [alpha] });
      if (cmd === "list_parked") return Promise.resolve({ type: "parked", paths: [] });
      if (cmd === "list_groups")
        return Promise.resolve({ type: "groups", order: [], members: {} });
      if (cmd === "wordpress_admin_users" && args?.site === "alpha") {
        return Promise.resolve(usersEnvelope(["editor"]));
      }
      return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    });

    const wrapper = await mountSites();
    await openEditFor(wrapper, alpha);

    const select = wrapper.find("#edit-wp-admin-user");
    expect(select.exists()).toBe(true);
    expect(select.attributes("disabled")).toBeUndefined();
    const labels = select.findAll("option").map((o) => o.text());
    expect(labels).toEqual(["Earliest admin (default)", "editor"]);
  });

  it("does not let a slow response for a previously-opened site overwrite the currently-open site's picker", async () => {
    const alpha = wpSite("alpha");
    const beta = wpSite("beta");
    const alphaUsers = deferred<unknown>();
    const betaUsers = deferred<unknown>();

    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_sites") return Promise.resolve({ type: "sites", sites: [alpha, beta] });
      if (cmd === "list_parked") return Promise.resolve({ type: "parked", paths: [] });
      if (cmd === "list_groups")
        return Promise.resolve({ type: "groups", order: [], members: {} });
      if (cmd === "wordpress_admin_users" && args?.site === "alpha") return alphaUsers.promise;
      if (cmd === "wordpress_admin_users" && args?.site === "beta") return betaUsers.promise;
      return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    });

    const wrapper = await mountSites();

    // Open alpha's dialog (kicks off its fetch, left in flight), then close it
    // and quickly open beta's dialog before alpha's fetch has resolved.
    await openEditFor(wrapper, alpha);
    await openEditFor(wrapper, beta);

    // beta's fetch resolves first...
    betaUsers.resolve(usersEnvelope(["beta-editor"]));
    await flushPromises();
    // ...then alpha's stale fetch finally resolves too.
    alphaUsers.resolve(usersEnvelope(["alpha-editor"]));
    await flushPromises();

    const select = wrapper.find("#edit-wp-admin-user");
    const labels = select.findAll("option").map((o) => o.text());
    expect(labels).toEqual(["Earliest admin (default)", "beta-editor"]);
  });

  it("shows an error state without touching the picker options when the fetch fails", async () => {
    const alpha = wpSite("alpha");
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_sites") return Promise.resolve({ type: "sites", sites: [alpha] });
      if (cmd === "list_parked") return Promise.resolve({ type: "parked", paths: [] });
      if (cmd === "list_groups")
        return Promise.resolve({ type: "groups", order: [], members: {} });
      if (cmd === "wordpress_admin_users" && args?.site === "alpha") {
        return Promise.reject(new Error("boom"));
      }
      return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    });

    const wrapper = await mountSites();
    await openEditFor(wrapper, alpha);

    const select = wrapper.find("#edit-wp-admin-user");
    expect(select.attributes("disabled")).toBeDefined();
    const labels = select.findAll("option").map((o) => o.text());
    expect(labels).toEqual(["Error: boom"]);
  });
});
