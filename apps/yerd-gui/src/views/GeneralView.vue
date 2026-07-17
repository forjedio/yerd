<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";

import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useDaemon } from "@/composables/useDaemon";
import { MIN_PORT, MAX_PORT, useFallbackPorts } from "@/composables/useFallbackPorts";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useToast } from "@/composables/useToast";
import {
  cliPathStatus,
  daemonInfo,
  dumpsStatus,
  getAutostart,
  getTrayIconVariant,
  installCliToPath,
  IpcError,
  openLoginItems,
  removeCliFromPath,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
  setMcpEnabled,
  setSymlinkProtection,
  setTrayIconVariant,
} from "@/ipc/client";
import type { AutostartState, CliPathStatus, TitleBarStyle, TrayIconVariant } from "@/ipc/types";
import { useTheme, type ThemePref } from "@/lib/theme";
import { useTitleBarStyle } from "@/lib/titleBarStyle";

const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();
const { pref, setTheme } = useTheme();
const { style: titleBarStyle, setTitleBarStyle } = useTitleBarStyle();

const busy = ref<string | null>(null);
const autostart = ref<AutostartState | null>(null);
// Host platform - drives macOS-specific daemon copy (on macOS the daemon runs
// as a background login item registered via SMAppService; see below) and
// whether the bundled `yerd` CLI on-PATH card is shown.
const { isMac, supportsPathInstall } = usePlatform();
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
  { value: "linux-reversed", label: "Linux (Reversed)" },
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
  try {
    await openLoginItems();
  } catch (e) {
    toast.error("Couldn't open Login Items", (e as IpcError).message);
  }
}

onMounted(() => {
  loadAutostart();
  void loadPlatform();
  loadCli();
  void loadTrayIconVariant();
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

/** Extract the port from a `host:port` socket address (e.g. `127.0.0.1:1053`). */
function portFromAddr(addr: string | undefined): number {
  if (!addr) return 0;
  const port = Number(addr.slice(addr.lastIndexOf(":") + 1));
  return Number.isInteger(port) ? port : 0;
}

async function loadApplicationPorts(): Promise<void> {
  // HTTP/HTTPS fallback + DNS come from `daemonInfo`; dumps from its own status
  // call; mail from the live status report.
  try {
    const info = await daemonInfo();
    fbHttp.value = info.fallback_http ? String(info.fallback_http) : "";
    fbHttps.value = info.fallback_https ? String(info.fallback_https) : "";
    // Prefer the configured `dns_port`; fall back to the *bound* DNS address
    // (always present, incl. on an older daemon that omits `dns_port`) so the
    // field shows a real value rather than relying on the placeholder.
    const dp = info.dns_port || portFromAddr(report.value?.dns_addr);
    dnsPort.value = dp ? String(dp) : "";
  } catch {
    // Daemon down / older daemon - clear so a later transient fetch failure
    // can't leave stale ports shown (or re-savable) under the "running" card.
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

// macOS pf redirect is pinned to the current HTTP/HTTPS ports, so block edits to
// those until the user un-elevates. Only fires on macOS (Linux reports null).
// DNS/mail/dumps are unaffected and stay editable.
const portsElevated = computed(() => report.value?.port_redirect === true);
const webPortsChanged = computed(
  () => fbHttp.value !== fbHttpSaved.value || fbHttps.value !== fbHttpsSaved.value,
);
const applicationPortsChanged = computed(
  () =>
    // While elevated the web ports are pinned and never submitted, so a stale
    // web delta must not flip Save on (or get snapshotted as if applied).
    (!portsElevated.value && webPortsChanged.value) ||
    dnsPort.value !== dnsPortSaved.value ||
    mailPort.value !== mailPortSaved.value ||
    dumpsPort.value !== dumpsPortSaved.value,
);

function openApplicationPorts(): void {
  // Pre-validate only the fields that changed (and are editable), so the confirm
  // modal never opens on input the daemon would reject anyway.
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

async function confirmApplicationPorts(close: () => void): Promise<void> {
  close();
  busy.value = "application-ports";
  try {
    const changes: Parameters<typeof applyAndRestart>[0] = {};
    // Omit HTTP/HTTPS while elevated (the redirect pins them) or unchanged.
    if (!portsElevated.value && webPortsChanged.value) {
      changes.web = { http: Number(fbHttp.value), https: Number(fbHttps.value) };
    }
    if (dnsPort.value !== dnsPortSaved.value) changes.dns = Number(dnsPort.value);
    if (mailPort.value !== mailPortSaved.value) changes.mail = Number(mailPort.value);
    if (dumpsPort.value !== dumpsPortSaved.value) changes.dumps = Number(dumpsPort.value);

    const res = await applyAndRestart(changes);
    if (res.ok) {
      // Re-snapshot the saved values so the form is no longer "changed". Only
      // re-snapshot web ports when they were actually submitted, so a pinned
      // (elevated) edit isn't silently recorded as applied.
      if (changes.web) {
        fbHttpSaved.value = fbHttp.value;
        fbHttpsSaved.value = fbHttps.value;
      }
      dnsPortSaved.value = dnsPort.value;
      mailPortSaved.value = mailPort.value;
      dumpsPortSaved.value = dumpsPort.value;
      toast.success("Ports updated", "Yerd restarted with the new ports.");
    } else {
      // Leave the user's edits in place so they can adjust and retry.
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

const MCP_CLAUDE_SNIPPET = "claude mcp add --scope user yerd -- yerd mcp";
const MCP_JSON_SNIPPET = JSON.stringify(
  { mcpServers: { yerd: { command: "yerd", args: ["mcp"] } } },
  null,
  2,
);

async function toggleMcp(on: boolean): Promise<void> {
  busy.value = "mcp";
  try {
    await setMcpEnabled(on);
    mcpEnabled.value = on;
    if (on) {
      toast.success(
        "AI agents enabled",
        "Register Yerd with your agent using the command below.",
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

async function copyMcpSnippet(snippet: string, label: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(snippet);
    toast.success(`${label} copied`);
  } catch {
    toast.error("Couldn't copy to the clipboard");
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

async function confirmDisableSymlinkProtection(close: () => void): Promise<void> {
  close();
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
  <div class="flex h-full flex-col">
    <PageHeader title="Settings" subtitle="Ports, startup, and appearance" />

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <!-- macOS: registered via SMAppService but awaiting Login-Items approval. -->
      <div
        v-if="autostart?.daemonPendingApproval"
        class="flex items-start justify-between gap-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4"
      >
        <div class="space-y-1">
          <p class="text-sm font-medium">Approve the Yerd background daemon</p>
          <p class="text-xs text-muted-foreground">
            Yerd is registered but waiting for you to enable it in System Settings
            → Login Items before it can serve your .test sites.
          </p>
        </div>
        <Button variant="outline" size="sm" @click="openApproval">
          Open Login Items
        </Button>
      </div>

      <!-- macOS: the GUI login item is registered but awaiting Login-Items approval. -->
      <div
        v-if="autostart?.guiPendingApproval"
        class="flex items-start justify-between gap-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4"
      >
        <div class="space-y-1">
          <p class="text-sm font-medium">Approve launching Yerd at login</p>
          <p class="text-xs text-muted-foreground">
            “Start the Yerd app at login” is set, but macOS needs you to enable it
            under System Settings → Login Items (Open at Login).
          </p>
        </div>
        <Button variant="outline" size="sm" @click="openApproval">
          Open Login Items
        </Button>
      </div>

      <!-- Application ports (HTTP/HTTPS fallback, DNS, mail, dumps) -->
      <Card v-if="running">
        <CardHeader>
          <CardTitle>Application Ports</CardTitle>
          <CardDescription>
            The loopback ports Yerd's services listen on. HTTP/HTTPS are the
            rootless ports used when 80/443 need elevation (must be
            {{ MIN_PORT }}+). Saving any change restarts the daemon.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <!-- Degraded: no web ports bound. -->
          <div
            v-if="report?.web_unbound"
            class="rounded-md border border-warning/40 bg-warning/10 p-3 text-sm"
          >
            <p class="font-medium">Yerd isn't serving any sites</p>
            <p class="mt-1 text-muted-foreground">
              It couldn't bind ports {{ report.web_unbound.http }}/{{
                report.web_unbound.https
              }}
              - they're in use by another process. Pick free HTTP/HTTPS ports
              below and save.
            </p>
          </div>
          <!-- Degraded: DNS port not bound. -->
          <div
            v-if="report?.dns_unbound != null"
            class="rounded-md border border-warning/40 bg-warning/10 p-3 text-sm"
          >
            <p class="font-medium">Yerd can't resolve .test domains</p>
            <p class="mt-1 text-muted-foreground">
              It couldn't bind DNS port {{ report.dns_unbound }} - it's in use by
              another process. Pick a free DNS port below and save. You may need
              to re-run Trust afterwards so the system points at the new port.
            </p>
          </div>
          <!-- Elevated: editing HTTP/HTTPS would break the pinned redirect. -->
          <p v-if="portsElevated" class="text-xs text-muted-foreground">
            HTTP/HTTPS ports are elevated, so they're pinned to the active
            redirect. Un-elevate ports (Doctor) before changing them. DNS, mail
            and dumps ports can still be changed.
          </p>
          <div class="grid grid-cols-2 gap-4 sm:grid-cols-3">
            <div class="space-y-1">
              <label for="ap-http" class="text-xs font-medium text-muted-foreground">HTTP</label>
              <Input
                id="ap-http"
                v-model="fbHttp"
                type="number"
                inputmode="numeric"
                :min="MIN_PORT"
                :max="MAX_PORT"
                :disabled="portsElevated || busy === 'application-ports'"
                aria-label="Rootless HTTP port"
                class="w-full font-mono"
                placeholder="8080"
              />
            </div>
            <div class="space-y-1">
              <label for="ap-https" class="text-xs font-medium text-muted-foreground">HTTPS</label>
              <Input
                id="ap-https"
                v-model="fbHttps"
                type="number"
                inputmode="numeric"
                :min="MIN_PORT"
                :max="MAX_PORT"
                :disabled="portsElevated || busy === 'application-ports'"
                aria-label="Rootless HTTPS port"
                class="w-full font-mono"
                placeholder="8443"
              />
            </div>
            <div class="space-y-1">
              <label for="ap-dns" class="text-xs font-medium text-muted-foreground">DNS</label>
              <Input
                id="ap-dns"
                v-model="dnsPort"
                type="number"
                inputmode="numeric"
                min="1"
                :max="MAX_PORT"
                :disabled="busy === 'application-ports'"
                aria-label="DNS responder port"
                class="w-full font-mono"
                placeholder="1053"
              />
            </div>
            <div class="space-y-1">
              <label for="ap-mail" class="text-xs font-medium text-muted-foreground">Mail (SMTP)</label>
              <Input
                id="ap-mail"
                v-model="mailPort"
                type="number"
                inputmode="numeric"
                min="1"
                :max="MAX_PORT"
                :disabled="busy === 'application-ports'"
                aria-label="Mail server port"
                class="w-full font-mono"
                placeholder="2525"
              />
            </div>
            <div class="space-y-1">
              <label for="ap-dumps" class="text-xs font-medium text-muted-foreground">Dumps</label>
              <Input
                id="ap-dumps"
                v-model="dumpsPort"
                type="number"
                inputmode="numeric"
                min="1"
                :max="MAX_PORT"
                :disabled="busy === 'application-ports'"
                aria-label="Dump server port"
                class="w-full font-mono"
                placeholder="2304"
              />
            </div>
          </div>
          <div class="flex justify-end">
            <Button
              size="sm"
              :disabled="!applicationPortsChanged || busy === 'application-ports'"
              @click="openApplicationPorts"
            >
              <Spinner v-if="busy === 'application-ports'" class="size-4" />
              Save &amp; restart
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Start at login -->
      <Card>
        <CardHeader>
          <CardTitle>Start at login</CardTitle>
          <CardDescription>Run Yerd automatically when you log in.</CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Run the Yerd daemon in the background</p>
              <p class="text-xs text-muted-foreground">
                {{ autostart?.daemonSupported === false
                  ? "Unavailable - no per-user service manager on this system."
                  : isMac
                    ? "Runs at login and serves your .test sites. Shows as “Yerd” in System Settings › Login Items. Use the tray Stop to stop it for this session; turn this off to keep it stopped."
                    : "Keeps your .test sites served after you log in." }}
              </p>
            </div>
            <Switch
              :model-value="autostart?.daemon ?? false"
              :disabled="busy === 'login:daemon' || autostart?.daemonSupported === false"
              aria-label="Run the Yerd daemon in the background"
              @update:model-value="toggleDaemonLogin"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Start the Yerd app at login</p>
              <p class="text-xs text-muted-foreground">Open this window when you log in.</p>
            </div>
            <Switch
              :model-value="autostart?.gui ?? false"
              :disabled="busy === 'login:gui'"
              aria-label="Start the Yerd app at login"
              @update:model-value="toggleGuiLogin"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Start the Yerd app minimized</p>
              <p class="text-xs text-muted-foreground">Launch hidden to the tray instead of showing the window.</p>
            </div>
            <Switch
              :model-value="autostart?.guiMinimized ?? false"
              :disabled="busy === 'login:gui-min' || !autostart?.gui"
              aria-label="Start the Yerd app minimized"
              @update:model-value="toggleGuiMinimized"
            />
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Security</CardTitle>
          <CardDescription>Control how the proxy treats symlinks inside your sites.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Symlink protection</p>
              <p id="symlink-protection-desc" class="text-xs text-muted-foreground">
                {{ symlinkProtection
                  ? "On - the proxy refuses to serve files reached through a symlink that resolves outside a site's own folder."
                  : "Off - the proxy will serve files reached through a symlink even when the target is outside a site's folder (e.g. a shared theme). Only turn this off for directories you trust; combined with a public tunnel it can expose files beyond the site root." }}
              </p>
            </div>
            <Switch
              :model-value="symlinkProtection"
              :disabled="busy === 'symlink-protection' || !connected"
              aria-label="Symlink protection"
              aria-describedby="symlink-protection-desc"
              @update:model-value="onSymlinkProtectionToggle"
            />
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>AI Agents (MCP)</CardTitle>
          <CardDescription>
            Let local AI agents manage Yerd through the Model Context Protocol.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Enable Yerd's MCP server</p>
              <p id="mcp-enabled-desc" class="text-xs text-muted-foreground">
                {{ mcpEnabled
                  ? "On - agents can list and create sites, manage PHP versions and proxies, and read captured mail and dumps. No tool deletes data. Turning this off applies to agent sessions started afterwards."
                  : "Off - agents can't use Yerd's tools. Turning it on takes effect on a running agent's next tool call." }}
              </p>
            </div>
            <Switch
              :model-value="mcpEnabled"
              :disabled="busy === 'mcp' || !connected"
              aria-label="Enable Yerd's MCP server"
              aria-describedby="mcp-enabled-desc"
              @update:model-value="toggleMcp"
            />
          </div>

          <div v-if="mcpEnabled" class="mt-4 border-t pt-4">
            <!-- Registering points the agent at the `yerd` binary by name, so
                 don't offer a command that can't resolve: on macOS that needs
                 the Terminal CLI shim below. -->
            <p v-if="mcpNeedsCliPath" class="text-xs text-muted-foreground">
              Install <code>yerd</code> on your PATH first (see Terminal CLI below), then come back
              here for the registration command.
            </p>
            <template v-else>
              <p class="text-sm font-medium">Register Yerd with your agent</p>
              <p class="mt-1 text-xs text-muted-foreground">
                Run this once for Claude Code:
              </p>
              <div class="mt-2 flex items-center gap-2">
                <code class="flex-1 truncate rounded bg-muted px-2 py-1 text-xs">{{ MCP_CLAUDE_SNIPPET }}</code>
                <Button
                  variant="outline"
                  size="sm"
                  @click="copyMcpSnippet(MCP_CLAUDE_SNIPPET, 'Command')"
                >
                  Copy
                </Button>
              </div>
              <p class="mt-3 text-xs text-muted-foreground">
                For other agents, add this to their MCP config:
              </p>
              <div class="mt-2 flex items-start gap-2">
                <pre class="flex-1 overflow-x-auto rounded bg-muted px-2 py-1 text-xs"><code>{{ MCP_JSON_SNIPPET }}</code></pre>
                <Button
                  variant="outline"
                  size="sm"
                  @click="copyMcpSnippet(MCP_JSON_SNIPPET, 'Config')"
                >
                  Copy
                </Button>
              </div>
            </template>
          </div>
        </CardContent>
      </Card>

      <!-- Terminal CLI (macOS + Linux). `yerd` itself is already on PATH on a
           packaged Linux install, but this also puts the PHP/tool shims dir on
           PATH, so it's still useful there. -->
      <Card v-if="supportsPathInstall">
        <CardHeader>
          <CardTitle>Terminal CLI</CardTitle>
          <CardDescription>Use the <code>yerd</code> command in your terminal.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Install <code>yerd</code> on your PATH</p>
              <p class="text-xs text-muted-foreground">
                {{ cli?.installed
                  ? "Installed - run `yerd` in a new terminal window."
                  : "Adds yerd and your installed tools (php, composer, ...) to your shell PATH." }}
              </p>
            </div>
            <Button
              variant="outline"
              size="sm"
              :disabled="busy === 'cli:path'"
              @click="toggleCliPath"
            >
              <Spinner v-if="busy === 'cli:path'" class="size-4" />
              {{ cli?.installed ? "Remove" : "Install" }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Appearance -->
      <Card>
        <CardHeader>
          <CardTitle>Appearance</CardTitle>
          <CardDescription>Theme and tray icon used by the Yerd app.</CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Theme</p>
              <p class="text-xs text-muted-foreground">
                Match your system, or force light or dark.
              </p>
            </div>
            <Select
              :model-value="pref"
              :options="themeOptions"
              aria-label="Theme"
              @update:model-value="(v: ThemePref) => setTheme(v)"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Tray icon</p>
              <p class="text-xs text-muted-foreground">
                Icon shown in the menu bar / system tray.
              </p>
            </div>
            <Select
              :model-value="trayIconVariant"
              :options="trayIconOptions"
              aria-label="Tray icon"
              @update:model-value="setTrayIconVariantPref"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Title bar</p>
              <p class="text-xs text-muted-foreground">
                Window control style used by the Yerd app.
              </p>
            </div>
            <Select
              :model-value="titleBarStyle"
              :options="titleBarStyleOptions"
              aria-label="Title bar"
              @update:model-value="setTitleBarStylePref"
            />
          </div>
        </CardContent>
      </Card>

    </div>

    <Modal v-model:open="applicationPortsOpen" title="Change application ports?">
      <p class="text-sm text-muted-foreground">
        Saving applies your port changes and restarts the daemon - this briefly
        stops all <strong class="text-foreground">.test</strong> sites, DNS, PHP
        pools, and this connection. It returns in a few seconds.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmApplicationPorts(close)">Save &amp; restart</Button>
      </template>
    </Modal>

    <Modal v-model:open="symlinkProtectionOffOpen" title="Turn off symlink protection?">
      <p class="text-sm text-muted-foreground">
        This is a global setting - it lowers protection for
        <strong class="text-foreground">every</strong> site, not just one. With it
        off, the proxy will serve any file reached through a symlink whose target
        sits outside a site's own folder, including symlinks left behind by
        dependencies or checked-out repos.
      </p>
      <p class="mt-2 text-sm text-muted-foreground">
        If a site is exposed over a public tunnel, those out-of-root files become
        reachable beyond your machine. Only turn this off for directories you trust.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmDisableSymlinkProtection(close)">Turn off protection</Button>
      </template>
    </Modal>
  </div>
</template>
