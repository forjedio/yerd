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
  installCliToPath,
  IpcError,
  openLoginItems,
  removeCliFromPath,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
} from "@/ipc/client";
import type { AutostartState, CliPathStatus } from "@/ipc/types";
import { useTheme, type ThemePref } from "@/lib/theme";

const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();
const { pref, setTheme } = useTheme();

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

const running = computed(() => connected.value === true);

// ── data loads ──
async function loadAutostart(): Promise<void> {
  try {
    autostart.value = await getAutostart();
  } catch (e) {
    toast.error("Couldn't load startup settings", (e as IpcError).message);
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
          <CardDescription>Theme used by the Yerd app.</CardDescription>
        </CardHeader>
        <CardContent>
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
  </div>
</template>
