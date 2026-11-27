<script setup lang="ts">
import {
  ArrowRight,
  CircleAlert,
  Download,
  Lock,
  Plug,
  RefreshCw,
  RotateCw,
  Trash2,
  TriangleAlert,
} from "lucide-vue-next";
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { RouterLink } from "vue-router";

import AboutSection from "@/components/AboutSection.vue";
import NetworkProxyCard from "@/components/NetworkProxyCard.vue";
import PageHeader from "@/components/PageHeader.vue";
import ConnectAgentDialog from "@/components/settings/ConnectAgentDialog.vue";
import DiagnosticsDialog from "@/components/settings/DiagnosticsDialog.vue";
import GuiLogsDialog from "@/components/settings/GuiLogsDialog.vue";
import MadeByForjed from "@/components/settings/MadeByForjed.vue";
import SettingRow from "@/components/settings/SettingRow.vue";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useDaemon } from "@/composables/useDaemon";
import { MIN_PORT, MAX_PORT, useFallbackPorts } from "@/composables/useFallbackPorts";
import { loadIdes, rescanIdes, useIdes } from "@/composables/useIdes";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useToast } from "@/composables/useToast";
import { useAboutRequests, useUpdateCheck } from "@/composables/useUpdateCheck";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  cliPathStatus,
  daemonInfo,
  dumpsStatus,
  getAutostart,
  getPreferredIde,
  getTrayIconVariant,
  installCliToPath,
  IpcError,
  openLoginItems,
  removeCliFromPath,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
  setMcpEnabled,
  setPreferredIde,
  setSymlinkProtection,
  setTrayIconVariant,
} from "@/ipc/client";
import type { AutostartState, CliPathStatus, TitleBarStyle, TrayIconVariant } from "@/ipc/types";
import { SYSTEM_LABEL } from "@/lib/ideChoice";
import { portFromAddr, settingsBanners } from "@/lib/settings";
import { useTheme } from "@/lib/theme";
import { useTitleBarStyle } from "@/lib/titleBarStyle";
import { toastOnError } from "@/lib/toastActions";
import { plural } from "@/lib/utils";

const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();
const updates = useUpdateCheck();
useAboutRequests(() => void updates.refreshCached());
const { pref, setTheme } = useTheme();
const { style: titleBarStyle, setTitleBarStyle } = useTitleBarStyle();

const busy = ref<string | null>(null);
const autostart = ref<AutostartState | null>(null);
// Host platform - drives macOS-specific daemon copy (on macOS the daemon runs
// as a background login item registered via SMAppService; see below) and
// whether the Terminal CLI and Preferred editor rows are shown.
const { isMac, supportsPathInstall } = usePlatform();
const { installedIdes } = useIdes();
const cli = ref<CliPathStatus | null>(null);

const themeOptions = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
] as const;

const trayIconVariant = ref<TrayIconVariant>("auto");
const trayIconOptions = [
  { value: "auto", label: "Automatic" },
  { value: "light-y", label: "Light Y" },
  { value: "dark-y", label: "Dark Y" },
  { value: "full", label: "Full icon" },
] as const;

const titleBarStyleOptions = [
  { value: "auto", label: "Automatic" },
  { value: "macos", label: "macOS" },
  { value: "linux", label: "Linux" },
  { value: "windows", label: "Windows" },
] as const;

async function setTitleBarStylePref(next: TitleBarStyle): Promise<void> {
  try {
    await setTitleBarStyle(next);
  } catch (e) {
    toast.error("Couldn't change the title bar style", (e as IpcError).message);
  }
}

const running = computed(() => connected.value === true);
const wide = useWideLayout();

const banners = computed(() =>
  settingsBanners(autostart.value, running.value ? report.value : null),
);

const connectAgentOpen = ref(false);
const logsOpen = ref(false);
const diagnosticsOpen = ref(false);

function onThemeChange(value: string): void {
  if (value === "system" || value === "light" || value === "dark") setTheme(value);
}

// ── data loads ──
async function loadAutostart(): Promise<void> {
  try {
    autostart.value = await getAutostart();
  } catch (e) {
    toast.error("Couldn't load startup settings", (e as IpcError).message);
  }
}

async function loadTrayIconVariant(): Promise<void> {
  try {
    trayIconVariant.value = await getTrayIconVariant();
  } catch (e) {
    toast.error("Couldn't load the tray icon setting", (e as IpcError).message);
  }
}

async function setTrayIconVariantPref(variant: TrayIconVariant): Promise<void> {
  const previous = trayIconVariant.value;
  trayIconVariant.value = variant;
  try {
    await setTrayIconVariant(variant);
  } catch (e) {
    trayIconVariant.value = previous;
    toast.error("Couldn't change the tray icon", (e as IpcError).message);
  }
}

// ── preferred editor ──
// The stored preference: "auto", "system", or an editor id. Kept raw so an id
// this host can't see (uninstalled, or set on another machine) survives untouched.
const preferredIde = ref("auto");

const preferredIdeOptions = computed(() => [
  { value: "auto", label: "Auto-detect" },
  ...installedIdes.value.map((ide) => ({ value: ide.id, label: ide.label })),
  { value: "system", label: SYSTEM_LABEL },
]);

// A native <select> renders blank when its value matches no option, so an
// unresolvable stored id shows as Auto-detect without rewriting the preference.
const preferredIdeSelection = computed(() =>
  preferredIdeOptions.value.some((o) => o.value === preferredIde.value)
    ? preferredIde.value
    : "auto",
);

async function loadPreferredIde(): Promise<void> {
  try {
    preferredIde.value = (await getPreferredIde()) ?? "auto";
  } catch (e) {
    toast.error("Couldn't load the preferred editor", (e as IpcError).message);
  }
}

async function setPreferredIdePref(next: string): Promise<void> {
  const previous = preferredIde.value;
  preferredIde.value = next;
  try {
    await setPreferredIde(next === "auto" ? null : next);
  } catch (e) {
    preferredIde.value = previous;
    toast.error("Couldn't change the preferred editor", (e as IpcError).message);
  }
}

/** A rescan usually changes nothing visible (the picker already lists what was
 *  found), so it confirms itself with a count. Kept brief: it's an idempotent
 *  action the user may click a few times while installing an editor, and a
 *  stack of identical toasts would be worse than none. */
const RESCAN_TOAST_MS = 1500;

async function rescanEditors(): Promise<void> {
  busy.value = "ide:rescan";
  try {
    const found = await rescanIdes();
    toast.success(
      found === 0 ? "No editors found" : `Found ${plural(found, "editor", "editors")}`,
      undefined,
      RESCAN_TOAST_MS,
    );
  } catch (e) {
    toast.error("Couldn't detect installed editors", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── CLI on PATH (macOS + Linux) + Login-Items approval ──
async function loadCli(): Promise<void> {
  try {
    cli.value = await cliPathStatus();
  } catch {
    cli.value = null;
  }
}

async function toggleCliPath(): Promise<void> {
  busy.value = "cli:path";
  try {
    if (cli.value?.installed) {
      await removeCliFromPath();
    } else {
      await installCliToPath();
    }
  } catch (e) {
    toast.error("Couldn't update the yerd CLI on PATH", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadCli();
  }
}

async function openApproval(): Promise<void> {
  await toastOnError("Couldn't open Login Items", () => openLoginItems());
}

onMounted(() => {
  loadAutostart();
  void loadPlatform();
  loadCli();
  void loadTrayIconVariant();
  void loadIdes();
  void loadPreferredIde();
  if (running.value) {
    void loadApplicationPorts();
  }
});

// Reload the editable port values whenever the daemon comes up.
watch(running, (up) => {
  if (up) void loadApplicationPorts();
});

// ── application ports (HTTP/HTTPS fallback, DNS, mail, dumps) ──
const { applyAndRestart, validate: validateFallbackPorts, validateLoopback } =
  useFallbackPorts();
const fbHttp = ref("");
const fbHttps = ref("");
const dnsPort = ref("");
const mailPort = ref("");
const dumpsPort = ref("");
// The values last loaded from / saved to the daemon, to detect a real change.
const fbHttpSaved = ref("");
const fbHttpsSaved = ref("");
const dnsPortSaved = ref("");
const mailPortSaved = ref("");
const dumpsPortSaved = ref("");
const applicationPortsOpen = ref(false);

/** The five port inputs in drawn order; `web` marks the HTTP/HTTPS pair. */
const portFields = [
  {
    id: "ap-http",
    label: "HTTP",
    model: fbHttp,
    aria: "Rootless HTTP port",
    placeholder: "8080",
    web: true,
  },
  {
    id: "ap-https",
    label: "HTTPS",
    model: fbHttps,
    aria: "Rootless HTTPS port",
    placeholder: "8443",
    web: true,
  },
  {
    id: "ap-dns",
    label: "DNS",
    model: dnsPort,
    aria: "DNS responder port",
    placeholder: "1053",
    web: false,
  },
  {
    id: "ap-mail",
    label: "Mail (SMTP)",
    model: mailPort,
    aria: "Mail server port",
    placeholder: "2525",
    web: false,
  },
  {
    id: "ap-dumps",
    label: "Dumps",
    model: dumpsPort,
    aria: "Dump server port",
    placeholder: "2304",
    web: false,
  },
];

/**
 * Loads the five ports and snapshots them as saved. HTTP/HTTPS and DNS come from
 * `daemonInfo`, dumps from its own status call, mail from the live report. DNS
 * prefers the configured `dns_port` and falls back to the bound address (always
 * present, even on an older daemon that omits `dns_port`). A failed fetch clears
 * its fields so stale ports can't be shown or re-saved.
 */
async function loadApplicationPorts(): Promise<void> {
  try {
    const info = await daemonInfo();
    fbHttp.value = info.fallback_http ? String(info.fallback_http) : "";
    fbHttps.value = info.fallback_https ? String(info.fallback_https) : "";
    const dp = info.dns_port || portFromAddr(report.value?.dns_addr);
    dnsPort.value = dp ? String(dp) : "";
  } catch {
    fbHttp.value = "";
    fbHttps.value = "";
    dnsPort.value = "";
  }
  try {
    const d = await dumpsStatus();
    dumpsPort.value = d.port ? String(d.port) : "";
  } catch {
    dumpsPort.value = "";
  }
  const mp = report.value?.mail?.port;
  mailPort.value = mp ? String(mp) : "";
  fbHttpSaved.value = fbHttp.value;
  fbHttpsSaved.value = fbHttps.value;
  dnsPortSaved.value = dnsPort.value;
  mailPortSaved.value = mailPort.value;
  dumpsPortSaved.value = dumpsPort.value;
}

// The mail port lives in the status report, which can lag the first paint by one
// poll. Fill it once it arrives, but only while the field is still untouched.
watch(
  () => report.value?.mail?.port,
  (p) => {
    if (p != null && mailPort.value === "" && mailPortSaved.value === "") {
      mailPort.value = String(p);
      mailPortSaved.value = String(p);
    }
  },
);

// Same for DNS against an older daemon that omits `dns_port` from `daemonInfo`:
// backfill from the bound `dns_addr` once the report arrives, while untouched.
watch(
  () => report.value?.dns_addr,
  (addr) => {
    const p = portFromAddr(addr);
    if (p && dnsPort.value === "" && dnsPortSaved.value === "") {
      dnsPort.value = String(p);
      dnsPortSaved.value = String(p);
    }
  },
);

// macOS port forwarder is pinned to the current HTTP/HTTPS ports, so lock those
// inputs until the user un-elevates (Doctor). Only fires on macOS (Linux reports null).
// DNS/mail/dumps are unaffected and stay editable.
const portsElevated = computed(() => report.value?.port_redirect === true);
const webPortsChanged = computed(
  () => fbHttp.value !== fbHttpSaved.value || fbHttps.value !== fbHttpsSaved.value,
);
/** While elevated the web ports are pinned and never submitted, so a stale web
 *  delta must not turn Save on (or get snapshotted as if applied). */
const applicationPortsChanged = computed(
  () =>
    (!portsElevated.value && webPortsChanged.value) ||
    dnsPort.value !== dnsPortSaved.value ||
    mailPort.value !== mailPortSaved.value ||
    dumpsPort.value !== dumpsPortSaved.value,
);

/** Pre-validates only the fields that changed (and are editable), so the
 *  confirm dialog never opens on input the daemon would reject anyway. */
function openApplicationPorts(): void {
  if (!portsElevated.value && webPortsChanged.value) {
    const err = validateFallbackPorts(Number(fbHttp.value), Number(fbHttps.value));
    if (err) {
      toast.error("Invalid ports", err);
      return;
    }
  }
  for (const [label, ref_, saved] of [
    ["DNS", dnsPort, dnsPortSaved],
    ["mail", mailPort, mailPortSaved],
    ["dumps", dumpsPort, dumpsPortSaved],
  ] as const) {
    if (ref_.value !== saved.value) {
      const err = validateLoopback(label, Number(ref_.value));
      if (err) {
        toast.error("Invalid ports", err);
        return;
      }
    }
  }
  void nextTick(() => {
    applicationPortsOpen.value = true;
  });
}

/**
 * Applies the changed ports and restarts the daemon. HTTP/HTTPS are omitted
 * while elevated (the redirect pins them) or unchanged, and are re-snapshotted
 * only when submitted so a pinned edit isn't recorded as applied. On failure
 * the user's edits stay in place to adjust and retry.
 */
async function confirmApplicationPorts(): Promise<void> {
  applicationPortsOpen.value = false;
  busy.value = "application-ports";
  try {
    const changes: Parameters<typeof applyAndRestart>[0] = {};
    if (!portsElevated.value && webPortsChanged.value) {
      changes.web = { http: Number(fbHttp.value), https: Number(fbHttps.value) };
    }
    if (dnsPort.value !== dnsPortSaved.value) changes.dns = Number(dnsPort.value);
    if (mailPort.value !== mailPortSaved.value) changes.mail = Number(mailPort.value);
    if (dumpsPort.value !== dumpsPortSaved.value) changes.dumps = Number(dumpsPort.value);

    const res = await applyAndRestart(changes);
    if (res.ok) {
      if (changes.web) {
        fbHttpSaved.value = fbHttp.value;
        fbHttpsSaved.value = fbHttps.value;
      }
      dnsPortSaved.value = dnsPort.value;
      mailPortSaved.value = mailPort.value;
      dumpsPortSaved.value = dumpsPort.value;
      toast.success("Ports updated", "Yerd restarted with the new ports.");
    } else {
      toast.error("Couldn't apply the new ports", res.message ?? "The daemon is still degraded.");
    }
  } finally {
    busy.value = null;
    await refreshStatus();
  }
}

// ── symlink protection (global proxy setting, from the status report) ──
// Default to protected (true) when the report hasn't arrived or predates the field.
const symlinkProtection = ref(true);
watch(
  () => report.value?.symlink_protection,
  (v) => {
    if (v !== undefined) symlinkProtection.value = v;
  },
  { immediate: true },
);

// ── MCP server (AI agents) ────────────────────────────────────────────────
// Opt-in, so default to off when the report hasn't arrived or predates the field.
const mcpEnabled = ref(false);
watch(
  () => report.value?.mcp_enabled,
  (v) => {
    if (v !== undefined) mcpEnabled.value = v;
  },
  { immediate: true },
);

// `yerd` reaches the shell PATH via the Terminal CLI shim on macOS; a packaged
// Linux install already puts it there, so only macOS gates on `cli.installed`.
const mcpNeedsCliPath = computed(() => isMac.value && !cli.value?.installed);

async function toggleMcp(on: boolean): Promise<void> {
  busy.value = "mcp";
  try {
    await setMcpEnabled(on);
    mcpEnabled.value = on;
    if (on) {
      toast.success(
        "AI agents enabled",
        "Register Yerd with your agent using the command.",
      );
    } else {
      toast.info(
        "AI agents disabled",
        "Agent sessions started from now on can't use Yerd's tools.",
      );
    }
    await refreshStatus();
  } catch (e) {
    toast.error("Couldn't change AI agent access", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

const symlinkProtectionOffOpen = ref(false);

// Enabling protection is safe and applies immediately; disabling it lowers a
// security boundary for every site, so route that direction through a confirm.
function onSymlinkProtectionToggle(on: boolean): void {
  if (on) {
    void toggleSymlinkProtection(true);
    return;
  }
  void nextTick(() => {
    symlinkProtectionOffOpen.value = true;
  });
}

async function toggleSymlinkProtection(on: boolean): Promise<void> {
  busy.value = "symlink-protection";
  try {
    await setSymlinkProtection(on);
    symlinkProtection.value = on;
    if (on) {
      toast.success(
        "Symlink protection enabled",
        "Symlinks resolving outside a site's root are blocked again.",
      );
    } else {
      toast.info(
        "Symlink protection disabled",
        "Symlinks resolving outside a site's root are now served for every site.",
      );
    }
    await refreshStatus();
  } catch (e) {
    toast.error("Couldn't change symlink protection", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function confirmDisableSymlinkProtection(): Promise<void> {
  symlinkProtectionOffOpen.value = false;
  await toggleSymlinkProtection(false);
}

// ── autostart toggles ──
async function toggleDaemonLogin(on: boolean): Promise<void> {
  busy.value = "login:daemon";
  try {
    await setAutostartDaemon(on);
  } catch (e) {
    toast.error("Couldn't change daemon autostart", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}

async function toggleGuiLogin(on: boolean): Promise<void> {
  busy.value = "login:gui";
  try {
    await setAutostartGui(on);
  } catch (e) {
    toast.error("Couldn't change app autostart", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}

async function toggleGuiMinimized(on: boolean): Promise<void> {
  busy.value = "login:gui-min";
  try {
    await setAutostartGuiMinimized(on);
  } catch (e) {
    toast.error("Couldn't change the minimized option", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <PageHeader title="Settings" />

    <div class="flex min-h-0 flex-1" :data-layout="wide ? 'wide' : 'compact'">
      <div
        :class="[
          'min-w-0 flex-1 space-y-5 overflow-y-auto pb-8',
          wide ? 'px-7 pt-6' : 'px-6 pt-5',
        ]"
      >
        <AboutSection
          v-if="!wide"
          layout="card"
          :updates="updates"
          @logs="logsOpen = true"
          @diagnostics="diagnosticsOpen = true"
        />

        <div
          v-for="b in banners"
          :key="b.id"
          :class="[
            'flex gap-3 rounded-[10px] border p-4',
            b.tone === 'warning'
              ? 'border-warning/40 bg-warning/10'
              : 'border-destructive/40 bg-destructive/10',
          ]"
          :data-testid="`banner-${b.id}`"
        >
          <TriangleAlert v-if="b.tone === 'warning'" class="mt-0.5 size-4 shrink-0 text-warning" />
          <CircleAlert v-else class="mt-0.5 size-4 shrink-0 text-destructive" />
          <div class="min-w-0">
            <p class="text-[13.5px] font-semibold">{{ b.title }}</p>
            <p class="mt-0.5 text-[12.5px] text-muted-foreground">{{ b.body }}</p>
            <Button
              v-if="b.loginItems"
              variant="outline"
              size="sm"
              class="mt-3"
              @click="openApproval"
            >
              Open Login Items
            </Button>
          </div>
        </div>

        <SettingsGroup label="Startup">
          <SettingRow label="Run the Yerd daemon at login">
            <template #description>
              {{ autostart?.daemonSupported === false
                ? "Unavailable - no per-user service manager on this system."
                : isMac
                  ? "Serves your .test sites from login. Listed as “Yerd” in Login Items."
                  : "Keeps your .test sites served after you log in." }}
            </template>
            <template #control>
              <Switch
                :model-value="autostart?.daemon ?? false"
                :disabled="busy === 'login:daemon' || autostart?.daemonSupported === false"
                aria-label="Run the Yerd daemon in the background"
                @update:model-value="toggleDaemonLogin"
              />
            </template>
          </SettingRow>
          <SettingRow label="Open the Yerd app at login">
            <template #description>Show this window when you log in.</template>
            <template #control>
              <Switch
                :model-value="autostart?.gui ?? false"
                :disabled="busy === 'login:gui'"
                aria-label="Start the Yerd app at login"
                @update:model-value="toggleGuiLogin"
              />
            </template>
          </SettingRow>
          <SettingRow label="Start minimized">
            <template #description>Launch hidden to the tray instead.</template>
            <template #control>
              <Switch
                :model-value="autostart?.guiMinimized ?? false"
                :disabled="busy === 'login:gui-min' || !autostart?.gui"
                aria-label="Start the Yerd app minimized"
                @update:model-value="toggleGuiMinimized"
              />
            </template>
          </SettingRow>
        </SettingsGroup>

        <SettingsGroup label="Appearance">
          <SettingRow label="Theme">
            <template #description>Match your system, or force light or dark.</template>
            <template #control>
              <SegmentedControl
                :model-value="pref"
                :options="themeOptions"
                size="md"
                variant="plain"
                aria-label="Theme"
                @update:model-value="onThemeChange"
              />
            </template>
          </SettingRow>
          <SettingRow label="Tray icon">
            <template #description>
              {{ isMac ? "Icon shown in the menu bar." : "Icon shown in the system tray." }}
            </template>
            <template #control>
              <Select
                :model-value="trayIconVariant"
                :options="trayIconOptions"
                aria-label="Tray icon"
                class="h-[34px] w-[200px]"
                @update:model-value="setTrayIconVariantPref"
              />
            </template>
          </SettingRow>
          <SettingRow label="Title bar">
            <template #description>Window control style.</template>
            <template #control>
              <Select
                :model-value="titleBarStyle"
                :options="titleBarStyleOptions"
                aria-label="Title bar"
                class="h-[34px] w-[200px]"
                @update:model-value="setTitleBarStylePref"
              />
            </template>
          </SettingRow>
          <!-- Same macOS-or-Linux predicate as the PATH install: host editor
               launching has no Windows adapter either. -->
          <SettingRow v-if="supportsPathInstall" label="Preferred editor">
            <template #description>Opened by a site's editor button. Sites can override it.</template>
            <template #control>
              <Select
                :model-value="preferredIdeSelection"
                :options="preferredIdeOptions"
                aria-label="Preferred editor"
                class="h-[34px] w-[200px]"
                @update:model-value="setPreferredIdePref"
              />
              <Button
                variant="outline"
                size="sm"
                class="h-[34px]"
                :disabled="busy === 'ide:rescan'"
                title="Look for installed editors again"
                @click="rescanEditors"
              >
                <Spinner v-if="busy === 'ide:rescan'" class="size-4" />
                <RefreshCw v-else />
                Rescan
              </Button>
            </template>
          </SettingRow>
        </SettingsGroup>

        <SettingsGroup v-if="running" label="Application ports">
          <div class="grid grid-cols-5 gap-3 px-[18px] pb-4 pt-4">
            <div
              v-for="field in portFields"
              :key="field.id"
              class="min-w-0 space-y-1.5"
            >
              <label :for="field.id" class="block truncate text-[12.5px] text-muted-foreground">
                {{ field.label }}
              </label>
              <div :class="['relative rounded-md', field.web && portsElevated && 'bg-sidebar']">
                <Lock
                  v-if="field.web && portsElevated"
                  class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
                  aria-hidden="true"
                />
                <Input
                  :id="field.id"
                  v-model="field.model.value"
                  type="number"
                  inputmode="numeric"
                  :min="field.web ? MIN_PORT : 1"
                  :max="MAX_PORT"
                  :disabled="
                    field.web
                      ? portsElevated || busy === 'application-ports'
                      : busy === 'application-ports'
                  "
                  :aria-label="field.aria"
                  :placeholder="field.placeholder"
                  :class="[
                    'h-[34px] font-mono',
                    field.web && portsElevated && 'pl-8 text-faint disabled:opacity-100',
                  ]"
                />
              </div>
            </div>
          </div>
          <div class="flex flex-wrap items-center gap-3 px-[18px] py-2.5">
            <p
              v-if="portsElevated"
              class="flex min-w-0 items-center gap-2 text-[12.5px] text-muted-foreground"
              data-testid="ports-elevated"
            >
              <Lock class="size-3.5 shrink-0 text-brand" aria-hidden="true" />
              HTTP and HTTPS are pinned to the 80/443 redirect.
              <RouterLink
                to="/doctor"
                class="inline-flex items-center gap-1 font-medium text-brand hover:underline"
              >
                Open Doctor <ArrowRight class="size-3.5" />
              </RouterLink>
            </p>
            <p v-else class="min-w-0 text-[12.5px] text-faint">
              Saving any change restarts the daemon. HTTP and HTTPS must be {{ MIN_PORT }} or above.
            </p>
            <Button
              size="sm"
              class="ml-auto"
              :disabled="!applicationPortsChanged || busy === 'application-ports'"
              @click="openApplicationPorts"
            >
              <Spinner v-if="busy === 'application-ports'" class="size-4" />
              <RotateCw v-else />
              Save &amp; restart
            </Button>
          </div>
        </SettingsGroup>

        <SettingsGroup label="Integrations and security">
          <SettingRow label="AI agents (MCP)" description-id="mcp-enabled-desc">
            <template #description>
              {{ mcpEnabled
                ? "Local agents can manage sites, PHP, proxies, mail and dumps. No tool deletes data."
                : "Agents can't use Yerd's tools. Turning it on takes effect on a running agent's next tool call." }}
            </template>
            <template #control>
              <Button
                v-if="mcpEnabled"
                variant="outline"
                size="sm"
                @click="connectAgentOpen = true"
              >
                <Plug /> Connect an agent
              </Button>
              <Switch
                :model-value="mcpEnabled"
                :disabled="busy === 'mcp' || !connected"
                aria-label="Enable Yerd's MCP server"
                aria-describedby="mcp-enabled-desc"
                @update:model-value="toggleMcp"
              />
            </template>
          </SettingRow>
          <!-- `yerd` itself is already on PATH on a packaged Linux install, but
               this also puts the PHP/tool shims dir on PATH, so it's still
               useful there. -->
          <SettingRow v-if="supportsPathInstall" label="Terminal CLI">
            <template #description>
              {{ cli?.installed
                ? "The yerd command is installed on your PATH."
                : "Adds yerd and your installed tools (php, composer, ...) to your shell PATH." }}
            </template>
            <template #control>
              <Button
                variant="outline"
                size="sm"
                :disabled="busy === 'cli:path'"
                @click="toggleCliPath"
              >
                <Spinner v-if="busy === 'cli:path'" class="size-4" />
                <Trash2 v-else-if="cli?.installed" />
                <Download v-else />
                {{ cli?.installed ? "Remove" : "Install" }}
              </Button>
            </template>
          </SettingRow>
          <SettingRow label="Symlink protection" description-id="symlink-protection-desc">
            <template #description>
              {{ symlinkProtection
                ? "Block files reached through symlinks that point outside a site's folder."
                : "The proxy will serve files reached through a symlink even when the target is outside a site's folder (e.g. a shared theme). Only turn this off for directories you trust; combined with a public tunnel it can expose files beyond the site root." }}
            </template>
            <template #control>
              <Switch
                :model-value="symlinkProtection"
                :disabled="busy === 'symlink-protection' || !connected"
                aria-label="Symlink protection"
                aria-describedby="symlink-protection-desc"
                @update:model-value="onSymlinkProtectionToggle"
              />
            </template>
          </SettingRow>
        </SettingsGroup>

        <NetworkProxyCard v-if="running" />

        <MadeByForjed v-if="!wide" class="pt-2 text-center" />
      </div>

      <aside
        v-if="wide"
        class="flex w-[302px] shrink-0 flex-col overflow-y-auto border-l bg-sidebar px-6 pb-5 pt-7"
      >
        <AboutSection
          layout="column"
          :updates="updates"
          @logs="logsOpen = true"
          @diagnostics="diagnosticsOpen = true"
        />
        <MadeByForjed class="mt-auto pt-6" />
      </aside>
    </div>

    <ConnectAgentDialog v-model:open="connectAgentOpen" :needs-cli-path="mcpNeedsCliPath" />
    <GuiLogsDialog v-model:open="logsOpen" />
    <DiagnosticsDialog v-model:open="diagnosticsOpen" />

    <DialogFrame
      v-model:open="applicationPortsOpen"
      title="Change application ports?"
      width="md"
    >
      <p class="text-[13px] text-muted-foreground">
        Saving applies your port changes and restarts the daemon. This briefly stops all
        <strong class="text-foreground">.test</strong> sites, DNS, PHP pools and this
        connection. It returns in a few seconds.
      </p>
      <template #footer>
        <Button variant="outline" @click="applicationPortsOpen = false">Cancel</Button>
        <Button @click="confirmApplicationPorts"><RotateCw /> Save &amp; restart</Button>
      </template>
    </DialogFrame>

    <DialogFrame
      v-model:open="symlinkProtectionOffOpen"
      title="Turn off symlink protection?"
      width="md"
    >
      <p class="text-[13px] text-muted-foreground">
        This lowers protection for <strong class="text-foreground">every</strong> site, not
        just one. The proxy will serve any file reached through a symlink whose target sits
        outside a site's folder.
      </p>
      <p class="mt-3 text-[13px] text-muted-foreground">
        If a site is shared over a public tunnel, those files become reachable beyond your
        machine. Only turn this off for directories you trust.
      </p>
      <template #footer>
        <Button variant="outline" @click="symlinkProtectionOffOpen = false">Cancel</Button>
        <Button variant="destructive" @click="confirmDisableSymlinkProtection">Turn off</Button>
      </template>
    </DialogFrame>
  </div>
</template>
