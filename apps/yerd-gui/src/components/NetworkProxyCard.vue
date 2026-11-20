<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { getNetworkSettings, IpcError, setNetworkSettings, testNetwork } from "@/ipc/client";
import type {
  NetworkSettings,
  NetworkTestResult,
  OutboundProxyMode,
  ProxySource,
} from "@/ipc/types";
import { cn } from "@/lib/utils";

const modeOptions = [
  { value: "system", label: "Detect automatically" },
  { value: "manual", label: "Manual proxy" },
  { value: "off", label: "No proxy" },
] as const;

const sourceLabels: Record<ProxySource, string> = {
  manual: "manual setting",
  daemon_env: "daemon environment",
  macos_system_settings: "macOS System Settings",
  gnome_settings: "GNOME settings",
  login_shell: "login shell",
  none: "none",
};

const settings = ref<NetworkSettings | null>(null);
const mode = ref<OutboundProxyMode>("system");
const httpProxy = ref("");
const httpsProxy = ref("");
const bypass = ref("");
const busy = ref<"load" | "save" | "test" | null>("load");
const error = ref<string | null>(null);
const result = ref<NetworkTestResult | null>(null);

function apply(s: NetworkSettings) {
  settings.value = s;
  mode.value = s.mode;
  httpProxy.value = s.http_proxy ?? "";
  httpsProxy.value = s.https_proxy ?? "";
  bypass.value = s.no_proxy.join(", ");
}

const detected = computed(() => {
  const e = settings.value?.effective;
  if (!e) return null;
  const proxy = e.https_proxy ?? e.http_proxy;
  if (!proxy) {
    return e.mode === "off" ? "Direct connection (proxy off)" : "No proxy detected";
  }
  return `${proxy} (${sourceLabels[e.source]})`;
});

const dirty = computed(() => {
  const s = settings.value;
  if (!s) return false;
  return (
    mode.value !== s.mode ||
    httpProxy.value.trim() !== (s.http_proxy ?? "") ||
    httpsProxy.value.trim() !== (s.https_proxy ?? "") ||
    bypassEntries().join(",") !== s.no_proxy.join(",")
  );
});

function bypassEntries(): string[] {
  return bypass.value
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

async function load() {
  busy.value = "load";
  error.value = null;
  try {
    apply(await getNetworkSettings());
  } catch (e) {
    error.value = (e as IpcError).message;
  } finally {
    busy.value = null;
  }
}

async function save() {
  busy.value = "save";
  error.value = null;
  result.value = null;
  const manual = mode.value === "manual";
  try {
    apply(
      await setNetworkSettings(
        mode.value,
        manual ? httpProxy.value.trim() : undefined,
        manual ? httpsProxy.value.trim() : undefined,
        bypassEntries(),
      ),
    );
  } catch (e) {
    error.value = (e as IpcError).message;
  } finally {
    busy.value = null;
  }
}

async function runTest() {
  busy.value = "test";
  error.value = null;
  result.value = null;
  try {
    result.value = await testNetwork();
  } catch (e) {
    error.value = (e as IpcError).message;
  } finally {
    busy.value = null;
  }
}

const resultRoute = computed(() => {
  const r = result.value;
  if (!r) return "";
  return r.via ? `via ${r.via} (${sourceLabels[r.source]})` : "direct";
});

onMounted(load);
</script>

<template>
  <SettingsGroup
    label="Network"
    note="Downloads, updates, Composer, npm, pnpm, Bun, the Laravel installer and WP-CLI"
  >
    <div class="flex items-center gap-4 px-[18px] py-3">
      <div class="min-w-0 flex-1">
        <p class="text-[13.5px] font-semibold">Proxy</p>
        <p class="mt-0.5 truncate text-[12.5px] text-muted-foreground" data-testid="network-detected">
          <template v-if="detected">In use: {{ detected }}</template>
          <template v-else-if="settings === null && busy === null">
            Couldn't load the network settings.
          </template>
          <template v-else>Loading…</template>
        </p>
      </div>
      <Select
        v-model="mode"
        :options="modeOptions"
        :disabled="busy !== null"
        aria-label="Proxy mode"
        class="w-[200px] shrink-0"
      />
    </div>

    <div class="space-y-3 px-[18px] py-3.5">
      <div v-if="mode === 'manual'" class="grid gap-3 sm:grid-cols-2">
        <label class="block space-y-1.5">
          <span class="block text-[12.5px] text-muted-foreground">HTTP proxy</span>
          <Input
            v-model="httpProxy"
            placeholder="http://user:pass@proxy.corp:3128"
            :disabled="busy !== null"
            class="font-mono"
            data-testid="network-http"
          />
        </label>
        <label class="block space-y-1.5">
          <span class="block text-[12.5px] text-muted-foreground">HTTPS proxy</span>
          <Input
            v-model="httpsProxy"
            placeholder="Same as HTTP"
            :disabled="busy !== null"
            class="font-mono"
            data-testid="network-https"
          />
        </label>
      </div>

      <label class="block space-y-1.5">
        <span class="block text-[12.5px] text-muted-foreground">Bypass the proxy for</span>
        <Input
          v-model="bypass"
          placeholder="corp.com, 10.0.0.0/8"
          :disabled="busy !== null"
          class="font-mono"
          data-testid="network-bypass"
        />
        <span class="block text-[11.5px] text-faint">
          Comma-separated. Localhost and your .test sites are always reached directly.
        </span>
      </label>

      <p v-if="error" class="text-[12.5px] text-destructive" data-testid="network-error">
        {{ error }}
      </p>
    </div>

    <div class="flex flex-wrap items-center gap-2 px-[18px] py-2.5">
      <span v-if="dirty" class="text-[12.5px] text-muted-foreground" data-testid="network-dirty">
        Save to test these settings
      </span>
      <div
        v-else-if="result"
        class="flex min-w-0 items-center gap-2 text-[12.5px]"
        data-testid="network-result"
      >
        <span
          :class="
            cn(
              'inline-flex h-5 shrink-0 items-center rounded px-2 text-[11px] font-medium',
              result.ok ? 'bg-success/15 text-success' : 'bg-destructive/15 text-destructive',
            )
          "
        >
          {{ result.ok ? "Connected" : "Failed" }}
        </span>
        <span class="min-w-0 truncate text-muted-foreground">
          <template v-if="result.status">HTTP {{ result.status }} </template>{{ resultRoute
          }}<template v-if="result.error">: {{ result.error }}</template>
        </span>
      </div>
      <div class="ml-auto flex items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          :disabled="busy !== null || dirty"
          data-testid="network-test"
          @click="runTest"
        >
          <Spinner v-if="busy === 'test'" class="size-4" />
          Test connection
        </Button>
        <Button
          v-if="settings === null && busy === null"
          size="sm"
          variant="outline"
          data-testid="network-retry"
          @click="load"
        >
          Retry
        </Button>
        <Button
          size="sm"
          :disabled="busy !== null || settings === null"
          data-testid="network-save"
          @click="save"
        >
          <Spinner v-if="busy === 'save'" class="size-4" />
          Save
        </Button>
      </div>
    </div>
  </SettingsGroup>
</template>
