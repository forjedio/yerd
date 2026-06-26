<script setup lang="ts">
import {
  ArrowLeft,
  ArrowRight,
  Check,
  Download,
  ExternalLink,
  FolderPlus,
  Rocket,
} from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import DaemonDiagnosticsPanel from "@/components/DaemonDiagnosticsPanel.vue";
import EnvironmentCard from "@/components/EnvironmentCard.vue";
import TitleBar from "@/components/TitleBar.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import logoUrl from "@/assets/logo.svg";
import { useDaemon } from "@/composables/useDaemon";
import { useDaemonStart } from "@/composables/useDaemonStart";
import { MIN_PORT, MAX_PORT, useFallbackPorts } from "@/composables/useFallbackPorts";
import { useOnboarding } from "@/composables/useOnboarding";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  cliPathStatus,
  getAutostart,
  hostPlatform,
  installCliToPath,
  installPhp,
  IpcError,
  openLoginItems,
  park,
  pickDirectory,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
} from "@/ipc/client";
import type { AutostartState, CliPathStatus, PhpVersion } from "@/ipc/types";

// A first-run guided setup, shown only on a never-set-up machine (see
// useOnboarding). Step 1 (start the daemon) is required; PHP, parking a folder,
// and elevation are each skippable. Finishing lands on the Overview.
const router = useRouter();
const toast = useToast();
const { connected, report, refresh } = useDaemon();
const { finish } = useOnboarding();

const STEPS = [
  { n: 1, label: "Daemon" },
  { n: 2, label: "PHP" },
  { n: 3, label: "Sites" },
  { n: 4, label: "Trust" },
  { n: 5, label: "Done" },
] as const;

const step = ref(1);

// ── step 1: daemon ──
// The start→wait→diagnose skeleton lives in the composable; onboarding injects
// its login-item ordering via `beforeProbe` (see installDaemon).
const {
  starting: daemonStarting,
  activeLabel: daemonStartLabel,
  pendingApproval,
  diagnostics,
  start: startDaemonFlow,
} = useDaemonStart();
const daemonUp = computed(() => connected.value === true);

// ── degraded ports (shown in step 1 when the daemon came up but couldn't bind
// its web ports and/or its DNS port) ──
const { applyAndRestart, validate: validateFallbackPorts, validateLoopback } =
  useFallbackPorts();
const fbHttp = ref("");
const fbHttps = ref("");
const dnsPort = ref("");
const fallbackBusy = ref(false);
const fallbackError = ref<string | null>(null);
const fallbackSkipped = ref(false);
const webUnbound = computed(() => report.value?.web_unbound ?? null);
// `dns_unbound` carries the configured DNS port that failed to bind - exactly
// the value the user wants to change, so seed the input from it.
const dnsUnbound = computed(() => report.value?.dns_unbound ?? null);
// Show the fix panel once the daemon is up AND reports a degraded web and/or DNS
// port, and the user hasn't skipped. Seed the inputs from the ports it tried.
const showPortFix = computed(
  () =>
    daemonUp.value &&
    (webUnbound.value != null || dnsUnbound.value != null) &&
    !fallbackSkipped.value,
);
watch(
  webUnbound,
  (u) => {
    if (u && !fbHttp.value) {
      fbHttp.value = String(u.http);
      fbHttps.value = String(u.https);
    }
  },
  { immediate: true },
);
watch(
  dnsUnbound,
  (p) => {
    if (p != null && !dnsPort.value) dnsPort.value = String(p);
  },
  { immediate: true },
);

async function savePortFix(): Promise<void> {
  fallbackError.value = null;
  const changes: Parameters<typeof applyAndRestart>[0] = {};
  if (webUnbound.value != null) {
    const err = validateFallbackPorts(Number(fbHttp.value), Number(fbHttps.value));
    if (err) {
      fallbackError.value = err;
      return;
    }
    changes.web = { http: Number(fbHttp.value), https: Number(fbHttps.value) };
  }
  if (dnsUnbound.value != null) {
    const err = validateLoopback("DNS", Number(dnsPort.value));
    if (err) {
      fallbackError.value = err;
      return;
    }
    changes.dns = Number(dnsPort.value);
  }
  fallbackBusy.value = true;
  try {
    const res = await applyAndRestart(changes);
    if (res.ok) {
      toast.success("Yerd is ready", "It restarted with the new ports.");
      // The `*_unbound` fields are now null, so the panel hides on its own.
    } else {
      fallbackError.value = res.message ?? "The daemon is still degraded.";
    }
  } finally {
    fallbackBusy.value = false;
  }
}

function skipPortFix(): void {
  fallbackSkipped.value = true;
}

async function installDaemon(): Promise<void> {
  await startDaemonFlow({
    // Suppress the per-call Login-Items nudge: enabling the daemon and the app
    // could each open System Settings, so we open it once in beforeProbe instead.
    nudge: false,
    beforeProbe: async () => {
      // Probe service support before enabling the login defaults.
      let autostart: AutostartState | null = null;
      try {
        autostart = await getAutostart();
      } catch {
        /* non-fatal */
      }
      // Onboarding default: run the daemon AND the app at login, app minimized to
      // the tray. Best-effort/idempotent - users change all three in Settings; a
      // missing service manager just skips the daemon toggle.
      await enableLoginDefaults(autostart?.daemonSupported ?? false);
      // Re-read AFTER enabling: the GUI login item is only registered now, so its
      // pending-approval state is meaningful. Open Login Items at most once if the
      // daemon OR the GUI needs approval.
      try {
        autostart = await getAutostart();
      } catch {
        /* non-fatal */
      }
      const pending =
        (autostart?.daemonPendingApproval ?? false) ||
        (autostart?.guiPendingApproval ?? false);
      // best-effort; don't block onboarding, and swallow any IPC rejection.
      if (pending) void openLoginItems().catch(() => {});
      return pending;
    },
  });
}

/**
 * Enable the onboarding login defaults: daemon at login, GUI at login, and
 * start-minimized. Each is best-effort so one failure (e.g. no per-user service
 * manager for the daemon toggle) never blocks onboarding; users can change all
 * three in Settings.
 */
async function enableLoginDefaults(daemonSupported: boolean): Promise<void> {
  if (daemonSupported) {
    try {
      // nudge=false: installDaemon opens Login Items once after all enables.
      await setAutostartDaemon(true, false);
    } catch {
      /* no service manager / best-effort */
    }
  }
  try {
    await setAutostartGui(true, false);
  } catch {
    /* best-effort */
  }
  try {
    await setAutostartGuiMinimized(true);
  } catch {
    /* best-effort */
  }
}

// Swallow any rejection from the async `openLoginItems` IPC call so a raw
// `@click` binding can't surface an unhandled promise rejection (best-effort UX).
function onOpenLoginItems(): void {
  void openLoginItems().catch(() => {});
}

// (Spinner/diagnostics auto-clear on connect is handled inside useDaemonStart.)
// No auto-advance - the user clicks Continue (enabled once `daemonUp`).

// ── step 2: PHP ──
const phpLoading = ref(false);
const phpInstalling = ref(false);
const phpOptions = ref<{ value: PhpVersion; label: string }[]>([]);
const selectedPhp = ref<PhpVersion>("");
const installedPhp = ref<PhpVersion | null>(null);

async function loadAvailablePhp(): Promise<void> {
  if (phpOptions.value.length || installedPhp.value) return;
  phpLoading.value = true;
  try {
    const r = await availablePhp();
    const have = new Set(r.installed);
    phpOptions.value = r.available
      .filter((v) => !have.has(v))
      .map((v) => ({ value: v, label: `PHP ${v}` }));
    // Preselect the latest (daemon returns ascending → last is newest).
    const opts = phpOptions.value;
    selectedPhp.value = opts[opts.length - 1]?.value ?? "";
    // Something already installed (e.g. revisiting) - reflect it.
    if (r.installed.length) {
      installedPhp.value = r.installed[r.installed.length - 1] ?? null;
    }
  } catch (e) {
    toast.error("Couldn't load PHP versions", (e as IpcError).message);
  } finally {
    phpLoading.value = false;
  }
}

async function doInstallPhp(): Promise<void> {
  const v = selectedPhp.value;
  if (!v) return;
  phpInstalling.value = true;
  try {
    await installPhp(v);
    installedPhp.value = v;
    await refresh();
    toast.success(`Installed PHP ${v}`, "It's set as your default.");
  } catch (e) {
    toast.error(`Install of PHP ${v} failed`, (e as IpcError).message);
  } finally {
    phpInstalling.value = false;
  }
}

// ── step 2: install yerd on PATH (macOS only; Linux already ships it on PATH
// via the .deb, so the button is hidden there - see `isMac`). Optional and
// recommended; it never blocks "Next". ──
const platform = ref("");
const isMac = computed(() => platform.value === "macos");
const cli = ref<CliPathStatus | null>(null);
const cliBusy = ref(false);

async function loadCliStatus(): Promise<void> {
  if (!isMac.value) return;
  try {
    cli.value = await cliPathStatus();
  } catch {
    cli.value = null;
  }
}

async function installCli(): Promise<void> {
  cliBusy.value = true;
  try {
    await installCliToPath();
    await loadCliStatus();
    toast.success("yerd is on your PATH", "Run `yerd` in a new terminal window.");
  } catch (e) {
    toast.error("Couldn't install the yerd CLI", (e as IpcError).message);
  } finally {
    cliBusy.value = false;
  }
}

onMounted(() => {
  hostPlatform()
    .then((p) => {
      platform.value = p;
      void loadCliStatus();
    })
    .catch(() => {});
});

// ── step 3: park a folder ──
const parking = ref(false);
const parkedDir = ref<string | null>(null);

async function doPark(): Promise<void> {
  const dir = await pickDirectory();
  if (!dir) return;
  parking.value = true;
  try {
    await park(dir);
    parkedDir.value = dir;
    await refresh();
    toast.success("Parked directory", dir);
  } catch (e) {
    toast.error("Park failed", (e as IpcError).message);
  } finally {
    parking.value = false;
  }
}

// ── navigation ──
watch(step, (s) => {
  if (s === 2) void loadAvailablePhp();
});

const forwardLabel = computed(() => {
  if (step.value === 1) return "Continue";
  if (step.value === 5) return "Get started";
  if (step.value === 2) return installedPhp.value ? "Continue" : "Skip for now";
  if (step.value === 3) return parkedDir.value ? "Continue" : "Skip for now";
  return "Continue";
});

const forwardDisabled = computed(() => step.value === 1 && !daemonUp.value);

const finishing = ref(false);

async function onForward(): Promise<void> {
  if (step.value < 5) {
    step.value += 1;
    return;
  }
  finishing.value = true;
  await finish();
  void router.push("/overview");
}

function onBack(): void {
  if (step.value > 1) step.value -= 1;
}
</script>

<template>
  <div class="flex h-full w-full flex-col bg-background">
    <TitleBar title="Welcome to Yerd" />

    <div class="flex min-h-0 flex-1 flex-col items-center overflow-y-auto px-8 py-10">
      <!-- `mx-auto` + `items-center` centre horizontally; `my-auto` centres the
           whole flow vertically when it fits, and collapses to scroll when not. -->
      <div class="mx-auto my-auto flex w-full max-w-xl flex-col">
        <!-- Brand + progress -->
        <div class="mb-8 flex flex-col items-center text-center">
          <img :src="logoUrl" alt="" class="size-12 rounded-xl" />
          <h1 class="mt-3 text-xl font-semibold tracking-tight">Welcome to Yerd</h1>
          <p class="mt-1 text-sm text-muted-foreground">
            A few quick steps to get your local PHP environment running.
          </p>
        </div>

        <ol class="mb-8 flex items-center justify-center gap-2">
          <li
            v-for="s in STEPS"
            :key="s.n"
            class="flex items-center gap-2"
          >
            <span
              class="flex size-7 items-center justify-center rounded-full border text-xs font-medium"
              :class="
                step > s.n
                  ? 'border-brand bg-brand text-brand-foreground'
                  : step === s.n
                    ? 'border-brand text-brand'
                    : 'border-border text-muted-foreground'
              "
            >
              <Check v-if="step > s.n" class="size-3.5" />
              <template v-else>{{ s.n }}</template>
            </span>
            <span
              v-if="s.n !== STEPS.length"
              class="h-px w-6"
              :class="step > s.n ? 'bg-brand' : 'bg-border'"
            />
          </li>
        </ol>

        <!-- Step body -->
        <div class="rounded-xl border bg-card p-6">
          <!-- 1. Daemon -->
          <section v-if="step === 1" class="space-y-4">
            <h2 class="text-base font-semibold">Install the Yerd daemon</h2>
            <p class="text-sm text-muted-foreground">
              <code>yerdd</code> is a small background service that supervises
              PHP-FPM, serves your <code>.test</code> sites over HTTP/HTTPS,
              answers DNS, and runs databases. It runs unprivileged - this app is
              just a client and never runs as root.
            </p>

            <div
              v-if="pendingApproval"
              class="rounded-md border border-warning/40 bg-warning/10 p-3 text-sm"
            >
              <p class="font-medium">One more step</p>
              <p class="mt-1 text-muted-foreground">
                macOS needs you to allow Yerd in the background. Enable it under
                Login Items, then it'll connect automatically.
              </p>
              <Button variant="outline" size="sm" class="mt-2" @click="onOpenLoginItems">
                <ExternalLink class="size-4" /> Open Login Items
              </Button>
            </div>

            <!-- Why the daemon didn't come up (hints + log/service details). -->
            <DaemonDiagnosticsPanel v-if="diagnostics" :diagnostics="diagnostics" />

            <!-- Degraded: the daemon is up but couldn't bind its web and/or DNS
                 ports. Let the user pick free ports (skippable). Stays visible
                 across its own restart via `fallbackBusy` so the action below
                 doesn't flash. -->
            <div
              v-if="showPortFix || fallbackBusy"
              class="space-y-3 rounded-md border border-warning/40 bg-warning/10 p-3 text-sm"
            >
              <div>
                <p class="font-medium">Yerd started, but isn't fully ready yet</p>
                <p v-if="webUnbound != null" class="mt-1 text-muted-foreground">
                  It couldn't bind its web ports - they're in use by another
                  program. Pick free ports ({{ MIN_PORT }} or higher, so they don't
                  need elevation) and Yerd will serve on those.
                </p>
                <p v-if="dnsUnbound != null" class="mt-1 text-muted-foreground">
                  It couldn't bind its DNS port {{ dnsUnbound }} - it's in use by
                  another program, so <strong class="text-foreground">.test</strong>
                  names won't resolve. Pick a free DNS port and restart.
                </p>
              </div>
              <div class="flex flex-wrap items-end gap-3">
                <div v-if="webUnbound != null" class="space-y-1">
                  <label for="ob-fb-http" class="text-xs font-medium text-muted-foreground">
                    HTTP
                  </label>
                  <Input
                    id="ob-fb-http"
                    v-model="fbHttp"
                    type="number"
                    inputmode="numeric"
                    :min="MIN_PORT"
                    :max="MAX_PORT"
                    :disabled="fallbackBusy"
                    aria-label="Rootless HTTP port"
                    class="w-24 font-mono"
                    placeholder="8080"
                  />
                </div>
                <div v-if="webUnbound != null" class="space-y-1">
                  <label for="ob-fb-https" class="text-xs font-medium text-muted-foreground">
                    HTTPS
                  </label>
                  <Input
                    id="ob-fb-https"
                    v-model="fbHttps"
                    type="number"
                    inputmode="numeric"
                    :min="MIN_PORT"
                    :max="MAX_PORT"
                    :disabled="fallbackBusy"
                    aria-label="Rootless HTTPS port"
                    class="w-24 font-mono"
                    placeholder="8443"
                  />
                </div>
                <div v-if="dnsUnbound != null" class="space-y-1">
                  <label for="ob-dns" class="text-xs font-medium text-muted-foreground">
                    DNS
                  </label>
                  <Input
                    id="ob-dns"
                    v-model="dnsPort"
                    type="number"
                    inputmode="numeric"
                    min="1"
                    :max="MAX_PORT"
                    :disabled="fallbackBusy"
                    aria-label="DNS responder port"
                    class="w-24 font-mono"
                    placeholder="1053"
                  />
                </div>
                <Button size="sm" :disabled="fallbackBusy" @click="savePortFix">
                  <Spinner v-if="fallbackBusy" class="size-4" />
                  Save &amp; validate
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  :disabled="fallbackBusy"
                  @click="skipPortFix"
                >
                  Skip for now
                </Button>
              </div>
              <p v-if="fallbackError" class="text-destructive">{{ fallbackError }}</p>
            </div>

            <!-- Action, below the content and right-aligned. -->
            <div class="flex justify-end">
              <div
                v-if="daemonUp"
                class="flex items-center gap-2 rounded-md bg-success/10 px-3 py-1.5 text-sm font-medium text-success"
              >
                <Check class="size-4" /> Running
              </div>
              <div
                v-else-if="fallbackBusy"
                class="flex items-center gap-2 rounded-md bg-muted px-3 py-1.5 text-sm font-medium text-muted-foreground"
              >
                <Spinner class="size-4" /> Restarting…
              </div>
              <Button v-else :disabled="daemonStarting" @click="installDaemon">
                <Spinner v-if="daemonStarting" class="size-4" />
                <Download v-else class="size-4" />
                {{ daemonStartLabel ?? "Install & start daemon" }}
              </Button>
            </div>
          </section>

          <!-- 2. PHP -->
          <section v-else-if="step === 2" class="space-y-4">
            <h2 class="text-base font-semibold">Install a PHP version</h2>
            <p class="text-sm text-muted-foreground">
              Pick a version to install - the latest is selected for you. The
              first version becomes your default. You can add more later.
              Downloads a prebuilt build; this can take a few minutes.
            </p>

            <div v-if="phpLoading" class="flex justify-center py-6">
              <Spinner class="size-5" />
            </div>
            <div
              v-else-if="installedPhp"
              class="flex items-center gap-2 rounded-md bg-success/10 px-3 py-2 text-sm text-success"
            >
              <Check class="size-4" /> PHP {{ installedPhp }} installed.
            </div>
            <template v-else-if="phpOptions.length">
              <Select
                class="w-full"
                :model-value="selectedPhp"
                :options="phpOptions"
                aria-label="PHP version to install"
                @update:model-value="(v: PhpVersion) => (selectedPhp = v)"
              />
              <div class="flex justify-end">
                <Button :disabled="phpInstalling || !selectedPhp" @click="doInstallPhp">
                  <Spinner v-if="phpInstalling" class="size-4" />
                  <Download v-else class="size-4" />
                  Install PHP {{ selectedPhp }}
                </Button>
              </div>
            </template>
            <p v-else class="text-sm text-muted-foreground">
              No installable versions were found. You can add one later from the
              PHP page.
            </p>

            <!-- Optional (recommended): put the `yerd` CLI on PATH. macOS only -
                 Linux already ships it via the .deb. Never blocks Continue. -->
            <div
              v-if="isMac"
              class="flex items-center justify-between gap-4 rounded-md border bg-muted/30 p-3"
            >
              <div>
                <p class="text-sm font-medium">
                  Install <code>yerd</code> on your PATH
                  <span class="text-muted-foreground">(recommended)</span>
                </p>
                <p class="text-xs text-muted-foreground">
                  {{
                    cli?.installed
                      ? "Installed - run `yerd` in a new terminal window."
                      : "Symlinks the bundled CLI so you can run `yerd` in your terminal."
                  }}
                </p>
              </div>
              <Button
                v-if="!cli?.installed"
                variant="outline"
                size="sm"
                :disabled="cliBusy"
                @click="installCli"
              >
                <Spinner v-if="cliBusy" class="size-4" />
                Install
              </Button>
              <div v-else class="flex items-center gap-1 text-sm font-medium text-success">
                <Check class="size-4" /> Installed
              </div>
            </div>
          </section>

          <!-- 3. Park a folder -->
          <section v-else-if="step === 3" class="space-y-4">
            <h2 class="text-base font-semibold">Park a projects folder</h2>
            <p class="text-sm text-muted-foreground">
              Point Yerd at a folder of projects. Each subfolder is served at
              <code>&lt;name&gt;.test</code> automatically.
            </p>

            <div
              v-if="parkedDir"
              class="flex items-center gap-2 rounded-md bg-success/10 px-3 py-2 text-sm text-success"
            >
              <Check class="size-4" /> Parked <span class="font-mono">{{ parkedDir }}</span>
            </div>
            <div v-else class="flex justify-end">
              <Button :disabled="parking" @click="doPark">
                <Spinner v-if="parking" class="size-4" />
                <FolderPlus v-else class="size-4" />
                Choose a folder…
              </Button>
            </div>
          </section>

          <!-- 4. Elevate -->
          <section v-else-if="step === 4" class="space-y-4">
            <h2 class="text-base font-semibold">Trust &amp; system access</h2>
            <p class="text-sm text-muted-foreground">
              For HTTPS on <code>.test</code> and serving on ports 80/443, Yerd
              needs to trust its local certificate authority, install a
              <code>.test</code> resolver, and bind privileged ports. You'll be
              asked for your password. This is optional - you can do it later
              from Doctor.
            </p>
            <EnvironmentCard />
          </section>

          <!-- 5. Done -->
          <section v-else class="space-y-4 text-center">
            <Rocket class="mx-auto size-10 text-brand" />
            <div>
              <h2 class="text-base font-semibold">You're all set</h2>
              <p class="mt-1 text-sm text-muted-foreground">
                Yerd is ready. Manage PHP, sites, and services from the dashboard.
              </p>
            </div>
          </section>
        </div>

        <!-- Nav -->
        <div class="mt-6 flex items-center justify-between">
          <Button v-if="step > 1" variant="ghost" :disabled="finishing" @click="onBack">
            <ArrowLeft class="size-4" /> Back
          </Button>
          <span v-else />
          <Button :disabled="forwardDisabled || finishing" @click="onForward">
            <Spinner v-if="finishing" class="size-4" />
            {{ forwardLabel }}
            <ArrowRight v-if="!finishing && step < 5" class="size-4" />
          </Button>
        </div>
      </div>
    </div>
  </div>
</template>
