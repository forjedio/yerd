<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";

import SiteNodePanel from "@/components/SiteNodePanel.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, wordpressAdminUsers } from "@/ipc/client";
import type { SiteEntry, StatusReport } from "@/ipc/types";
import { comparePhpVersions } from "@/lib/phpVersion";
import { frameworkDetail, frameworkLabel, sourceDetail } from "@/lib/sites";

/**
 * The inspector's ENVIRONMENT card: PHP and Node versions, HTTPS, WordPress
 * auto-login (WordPress only), the detected framework and where the site comes
 * from. Writes go up as events; the parent owns the mutations.
 */
const props = defineProps<{
  site: SiteEntry;
  report: StatusReport | null;
  phpVersions: string[];
  defaultPhp: string;
  home: string | null;
  /** The parked root serving this site, or null for a linked/unmatched one. */
  parentDir: string | null;
  busy?: boolean;
}>();

const emit = defineEmits<{
  changePhp: [site: SiteEntry, version: string];
  toggleSecure: [site: SiteEntry];
  /** `options.silent` suppresses the parent's success toast, for a change the
   *  user didn't ask for and that another toast already explains. */
  changeWpAutoLogin: [
    site: SiteEntry,
    enabled: boolean,
    user: string | null,
    options?: { silent?: boolean },
  ];
  nodeChanged: [];
}>();

const toast = useToast();

/** Above this many versions the segmented control would overflow the row. */
const SEGMENTED_MAX = 6;

const phpOptions = computed(() =>
  Array.from(new Set([props.site.php, ...props.phpVersions]))
    .sort(comparePhpVersions)
    .map((version) => ({ value: version, label: version })),
);

const phpSelectOptions = computed(() =>
  phpOptions.value.map((o) => ({ value: o.value, label: `PHP ${o.value}` })),
);

const phpSubtitle = computed(() => {
  if (!props.defaultPhp) return "";
  return props.site.php === props.defaultPhp
    ? `Same as the global default (${props.defaultPhp})`
    : `Global default is ${props.defaultPhp}`;
});

const httpsSubtitle = computed(() => {
  if (!props.site.secure) return "Served over plain HTTP";
  const trusted = props.report?.ca.trusted_system;
  const state = trusted === true ? "trusted" : trusted === false ? "not trusted" : "trust unknown";
  return `Certificate issued by Yerd local CA · ${state}`;
});

function changePhp(version: string): void {
  if (version !== props.site.php) emit("changePhp", props.site, version);
}

const DEFAULT_ADMIN_OPTION = { value: "", label: "Earliest admin (default)" };
type WpAdminUsersStatus = "idle" | "loading" | "ready" | "error";
const wpAdminUsersStatus = ref<WpAdminUsersStatus>("idle");
const wpAdminUsersOptions = ref<{ value: string; label: string }[]>([DEFAULT_ADMIN_OPTION]);

/** True while a "switch auto-login on" click is waiting on the admin list, so
 *  the switch can't be clicked again mid-check. */
const wpAutoLoginChecking = ref(false);

/** Bumped on every `loadWpAdminUsers` call so a response for a site the user
 *  has since navigated away from (closed the inspector, selected another site)
 *  can recognize it's stale and skip applying its result. */
let wpAdminUsersRequestId = 0;

/** Fetch `name`'s admin list, reporting whether it is now loaded. A stale
 *  response (the user moved on) counts as a failure: it must not be acted on.
 *
 *  On failure a site already set to auto-login is switched back off, since
 *  auto-login can't work without an admin to mint the token for. That write is
 *  silent - the error toast is the only message worth showing for one failure -
 *  and only happens for the site still in view. */
async function loadWpAdminUsers(name: string): Promise<boolean> {
  const requestId = ++wpAdminUsersRequestId;
  wpAdminUsersStatus.value = "loading";
  try {
    const users = await wordpressAdminUsers(name);
    if (requestId !== wpAdminUsersRequestId) return false;
    wpAdminUsersOptions.value = [
      DEFAULT_ADMIN_OPTION,
      ...users.map((u) => ({ value: u.login, label: u.display_name || u.login })),
    ];
    wpAdminUsersStatus.value = "ready";
    return true;
  } catch (e) {
    if (requestId !== wpAdminUsersRequestId) return false;
    wpAdminUsersStatus.value = "error";
    toast.error(
      "Couldn't load WordPress admin users",
      (e as IpcError).message || "couldn't load admin users",
    );
    if (props.site.name === name && props.site.wp_auto_login) {
      emit("changeWpAutoLogin", props.site, false, null, { silent: true });
    }
    return false;
  }
}

/** Switching auto-login on is gated on the admin list, retrying a fetch that
 *  failed earlier: the daemon needs a user to mint the token for, so the write
 *  only happens once the list is in hand. A failure toasts once and leaves the
 *  switch off, rather than writing "on" and immediately writing "off" again -
 *  which is three toasts for one click. Switching off needs no such check. */
async function toggleWpAutoLogin(site: SiteEntry, enabled: boolean): Promise<void> {
  const user = site.wp_auto_login_user || null;
  if (!enabled) {
    emit("changeWpAutoLogin", site, false, user);
    return;
  }
  wpAutoLoginChecking.value = true;
  try {
    if (wpAdminUsersStatus.value !== "ready" && !(await loadWpAdminUsers(site.name))) return;
  } finally {
    wpAutoLoginChecking.value = false;
  }
  if (props.site.name !== site.name) return;
  emit("changeWpAutoLogin", site, true, user);
}

function changeWpAutoLoginUser(site: SiteEntry, user: string): void {
  emit("changeWpAutoLogin", site, true, user || null);
}

/** Retargeting refetches the WordPress admin list for the site now in view, but
 *  only where auto-login is on: opening a site that isn't using it must not
 *  reach for the database (nor report that it couldn't). The request id is
 *  invalidated and the state cleared first, so a fetch left live across a
 *  switch can't toast, or write auto-login off, behind the user's back. */
watch(
  () => props.site.name,
  () => {
    wpAdminUsersRequestId += 1;
    wpAdminUsersStatus.value = "idle";
    wpAdminUsersOptions.value = [DEFAULT_ADMIN_OPTION];
    if (!props.site.is_wordpress || !props.site.wp_auto_login) return;
    void loadWpAdminUsers(props.site.name);
  },
  { immediate: true },
);

onUnmounted(() => {
  wpAdminUsersRequestId += 1;
});

const ROW = "flex min-h-14 items-center justify-between gap-4 px-4 py-2.5";
</script>

<template>
  <section>
    <h3 class="mb-2 px-1 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">
      Environment
    </h3>
    <div class="divide-y rounded-[10px] border bg-card" data-testid="environment-card">
      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">PHP version</p>
          <p class="truncate text-[11px] text-faint">{{ phpSubtitle }}</p>
        </div>
        <SegmentedControl
          v-if="phpOptions.length <= SEGMENTED_MAX"
          :model-value="site.php"
          :options="phpOptions"
          aria-label="Site PHP version"
          :disabled="busy"
          @update:model-value="changePhp"
        />
        <Select
          v-else
          :model-value="site.php"
          :options="phpSelectOptions"
          aria-label="Site PHP version"
          :disabled="busy"
          @update:model-value="changePhp"
        />
      </div>

      <SiteNodePanel :site="site" @changed="emit('nodeChanged')" />

      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">HTTPS</p>
          <p class="truncate text-[11px] text-faint" data-testid="https-subtitle">
            {{ httpsSubtitle }}
          </p>
        </div>
        <Switch
          :model-value="site.secure"
          :disabled="busy"
          aria-label="HTTPS"
          @update:model-value="emit('toggleSecure', site)"
        />
      </div>

      <div v-if="site.is_wordpress" class="px-4 py-2.5">
        <div class="flex min-h-9 items-center justify-between gap-4">
          <div class="min-w-0">
            <p class="text-[13px] font-semibold">WordPress auto-login</p>
            <p class="text-[11px] text-faint">Sign in automatically when opening WP Admin.</p>
          </div>
          <Switch
            :model-value="site.wp_auto_login ?? false"
            :disabled="busy || wpAutoLoginChecking"
            aria-label="WordPress Auto Admin Login"
            @update:model-value="toggleWpAutoLogin(site, $event)"
          />
        </div>
        <div
          v-if="site.wp_auto_login && wpAdminUsersStatus === 'ready'"
          class="mt-2 flex items-center justify-between gap-4"
        >
          <label class="text-xs text-muted-foreground" for="site-wp-admin-user">Sign in as</label>
          <Select
            id="site-wp-admin-user"
            :model-value="site.wp_auto_login_user ?? ''"
            :options="wpAdminUsersOptions"
            :disabled="busy"
            aria-label="Sign in as"
            @update:model-value="changeWpAutoLoginUser(site, $event)"
          />
        </div>
      </div>

      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Framework</p>
          <p class="truncate text-[11px] text-faint">{{ frameworkDetail(site) }}</p>
        </div>
        <span
          class="shrink-0 text-[13px] font-medium"
          :class="frameworkLabel(site) === 'Laravel' ? 'text-destructive' : 'text-foreground'"
          data-testid="framework-label"
        >
          {{ frameworkLabel(site) }}
        </span>
      </div>

      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Source</p>
          <p class="truncate text-[11px] text-faint">{{ sourceDetail(site, parentDir, home) }}</p>
        </div>
        <span
          class="shrink-0 rounded-md border bg-muted px-2 py-0.5 text-xs text-muted-foreground"
          data-testid="source-chip"
        >
          {{ site.kind === "linked" ? "Linked" : "Parked" }}
        </span>
      </div>
    </div>
  </section>
</template>
