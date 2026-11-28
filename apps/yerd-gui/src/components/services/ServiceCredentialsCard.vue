<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { Copy, Eye, EyeOff, KeyRound } from "lucide-vue-next";

import { CARD } from "@/components/ui/styles";
import { useToast } from "@/composables/useToast";
import type { S3PublicCredentials } from "@/composables/useS3Storage";
import { IpcError, s3Credentials } from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { SECRET_MASK } from "@/lib/services";
import { copyWithToast } from "@/lib/toastActions";
import { cn } from "@/lib/utils";

/**
 * The S3 inspector's Credentials card: the endpoint, access key and secret
 * key generated for this install, each with a copy button. The secret stays
 * masked until the eye reveals it; it is fetched only on reveal or copy, kept
 * in this card alone, and dropped on hide, unmount or a service change.
 */
const props = defineProps<{
  service: ServiceStatus;
  credentials: S3PublicCredentials | null;
}>();

const toast = useToast();

const secret = ref<string | null>(null);
const fetching = ref(false);

const endpoint = computed(() => props.credentials?.endpoint ?? `http://127.0.0.1:${props.service.port}`);
const accessKey = computed(() => props.credentials?.access_key ?? "");

watch(
  () => props.service.service,
  () => {
    secret.value = null;
  },
);

onBeforeUnmount(() => {
  secret.value = null;
});

async function fetchSecret(): Promise<string | null> {
  fetching.value = true;
  try {
    const c = await s3Credentials(true);
    return c.secret_key ?? null;
  } catch (e) {
    toast.error("Couldn't read the secret key", (e as IpcError).message);
    return null;
  } finally {
    fetching.value = false;
  }
}

async function toggleSecret(): Promise<void> {
  if (secret.value !== null) {
    secret.value = null;
    return;
  }
  secret.value = await fetchSecret();
}

async function copy(text: string): Promise<void> {
  await copyWithToast(text, { title: "Copied to clipboard" });
}

async function copySecret(): Promise<void> {
  const value = secret.value ?? (await fetchSecret());
  if (value !== null) await copy(value);
}

const FIELD_ROW = "flex items-center gap-2 px-3.5 py-2";
const LABEL = "block text-[10px] font-medium uppercase leading-4 tracking-[0.6px] text-faint";
const VALUE = "mt-0.5 block truncate font-mono text-[13px] leading-5";
const ICON_BTN =
  "flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
</script>

<template>
  <section :class="cn(CARD, 'p-4')" data-testid="credentials-card">
    <h3 class="flex items-center gap-2 text-sm font-semibold">
      <KeyRound class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      Credentials
    </h3>
    <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">
      Generated for this install. Any S3 client or SDK on this machine can use them.
    </p>

    <div class="mt-3 divide-y rounded-md border bg-sidebar">
      <div :class="FIELD_ROW">
        <div class="min-w-0 flex-1">
          <span :class="LABEL">Endpoint</span>
          <span :class="VALUE" data-testid="cred-endpoint">{{ endpoint }}</span>
        </div>
        <button type="button" :class="ICON_BTN" aria-label="Copy endpoint" @click="copy(endpoint)">
          <Copy class="size-3.5" />
        </button>
      </div>
      <div :class="FIELD_ROW">
        <div class="min-w-0 flex-1">
          <span :class="LABEL">Access key</span>
          <span :class="VALUE" data-testid="cred-access">{{ accessKey || "…" }}</span>
        </div>
        <button
          type="button"
          :class="ICON_BTN"
          aria-label="Copy access key"
          :disabled="!accessKey"
          @click="copy(accessKey)"
        >
          <Copy class="size-3.5" />
        </button>
      </div>
      <div :class="FIELD_ROW">
        <div class="min-w-0 flex-1">
          <span :class="LABEL">Secret key</span>
          <span :class="VALUE" data-testid="cred-secret">{{ secret ?? SECRET_MASK }}</span>
        </div>
        <button
          type="button"
          :class="ICON_BTN"
          :aria-label="secret === null ? 'Reveal secret key' : 'Hide secret key'"
          :aria-pressed="secret !== null"
          :disabled="fetching"
          @click="toggleSecret"
        >
          <EyeOff v-if="secret !== null" class="size-3.5" />
          <Eye v-else class="size-3.5" />
        </button>
        <button
          type="button"
          :class="ICON_BTN"
          aria-label="Copy secret key"
          :disabled="fetching"
          @click="copySecret"
        >
          <Copy class="size-3.5" />
        </button>
      </div>
    </div>
  </section>
</template>
