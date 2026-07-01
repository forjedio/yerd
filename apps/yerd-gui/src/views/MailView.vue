<script setup lang="ts">
import { Copy } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";

import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
import type { Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Input from "@/components/ui/Input.vue";
import Switch from "@/components/ui/Switch.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { IpcError, setMailEnabled, showMailsWindow } from "@/ipc/client";

const toast = useToast();
const { report, refresh } = useDaemon();

// Live mail status comes from the shared 4s status poll (no extra loop).
const mail = computed(() => report.value?.mail ?? null);

onUnmounted(registerViewActions({ refresh: () => void refresh() }));

// Local editable copy of the enable toggle, seeded from the live status. The
// mail *port* is now edited centrally on the Settings ▸ Application Ports page.
const enabled = ref(false);
const busy = ref(false);

watch(
  mail,
  (m) => {
    if (!m) return;
    enabled.value = m.enabled;
  },
  { immediate: true },
);

const statusTone = computed<Tone>(() => {
  const m = mail.value;
  if (!m || !m.enabled) return "muted";
  return m.listening ? "ok" : "warn";
});

const statusLabel = computed(() => {
  const m = mail.value;
  if (!m) return "Unknown";
  if (!m.enabled) return "Disabled";
  return m.listening ? "Running" : "Enabled - port unavailable";
});

async function onToggleEnabled(next: boolean): Promise<void> {
  busy.value = true;
  try {
    await setMailEnabled(next);
    enabled.value = next;
    toast.success(
      next ? "Mail capture enabled" : "Mail capture disabled",
      "Takes effect after the daemon restarts.",
    );
    await refresh();
  } catch (e) {
    toast.error("Couldn't update mail capture", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}

async function onShowMails(): Promise<void> {
  try {
    await showMailsWindow();
  } catch (e) {
    toast.error("Couldn't open the mail viewer", (e as IpcError).message);
  }
}

// ── Laravel .env snippet ──
// Sensible Laravel defaults; the "From name" follows the Laravel idiom of
// referencing the app's APP_NAME, and the port tracks the live mail server.
const fromName = ref("");
const fromAddress = ref("");
const mailPort = computed(() => mail.value?.port ?? 2525);

const envSnippet = computed(() => {
  const name = fromName.value.trim() || "${APP_NAME}";
  const address = fromAddress.value.trim() || "hello@example.com";
  return [
    "MAIL_MAILER=smtp",
    "MAIL_HOST=127.0.0.1",
    `MAIL_PORT=${mailPort.value}`,
    "MAIL_USERNAME=null",
    "MAIL_PASSWORD=null",
    "MAIL_ENCRYPTION=null",
    `MAIL_FROM_ADDRESS="${address}"`,
    `MAIL_FROM_NAME="${name}"`,
  ].join("\n");
});

async function copyEnv(): Promise<void> {
  try {
    await navigator.clipboard.writeText(envSnippet.value);
    toast.success("Copied to clipboard", "Paste these into your Laravel .env file.");
  } catch {
    toast.error("Couldn't copy");
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Mail"
      subtitle="Capture and inspect emails your apps send during development"
      docs="/guide/mail"
    />

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Mail Server</CardTitle>
          <CardDescription>
            Yerd runs a local SMTP server that captures every outgoing email so
            you can preview it here. Point your app's mailer at
            <code>127.0.0.1</code> on the port below.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-5">
          <!-- Status -->
          <div class="flex items-center justify-between border-b pb-4">
            <div>
              <p class="text-sm font-medium">Mail server status</p>
              <p class="text-xs text-muted-foreground">
                {{ mail?.count ?? 0 }} captured email(s)
              </p>
            </div>
            <StatusPill
              :tone="statusTone"
              :label="statusLabel"
              :pulse="statusTone === 'ok'"
            />
          </div>

          <!-- Enable toggle -->
          <div class="flex items-center justify-between">
            <div>
              <p class="text-sm font-medium">Enabled</p>
              <p class="text-xs text-muted-foreground">
                Start the capture server when the daemon boots.
              </p>
            </div>
            <Switch
              :model-value="enabled"
              :disabled="busy"
              aria-label="Enable mail capture"
              @update:model-value="onToggleEnabled"
            />
          </div>

          <!-- Port (read-only; edited centrally on Settings ▸ Application Ports) -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Mail server port</p>
              <p class="text-xs text-muted-foreground">
                Change it in Settings ▸ Application Ports.
              </p>
            </div>
            <span class="font-mono text-sm">{{ mailPort }}</span>
          </div>

          <!-- Actions -->
          <div class="flex items-center justify-end border-t pt-4">
            <Button @click="onShowMails">Show Mails</Button>
          </div>
        </CardContent>
      </Card>

      <!-- Laravel .env configuration -->
      <Card>
        <CardHeader>
          <CardTitle>Laravel configuration</CardTitle>
          <CardDescription>
            Add these to your Laravel app's <code>.env</code> to route its mail
            through Yerd's capture server.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="grid gap-3 sm:grid-cols-2">
            <div>
              <label class="text-sm font-medium" for="mail-from-name">From name</label>
              <Input
                id="mail-from-name"
                v-model="fromName"
                class="mt-1.5"
                placeholder="${APP_NAME}"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                Defaults to your app's <code>APP_NAME</code>.
              </p>
            </div>
            <div>
              <label class="text-sm font-medium" for="mail-from-address">From address</label>
              <Input
                id="mail-from-address"
                v-model="fromAddress"
                class="mt-1.5"
                placeholder="hello@example.com"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                Capture accepts any address - nothing is actually delivered.
              </p>
            </div>
          </div>

          <div class="relative">
            <pre class="overflow-x-auto rounded-md border bg-muted/50 p-3 pr-12 font-mono text-xs leading-relaxed text-foreground">{{ envSnippet }}</pre>
            <Button
              variant="ghost"
              size="icon"
              class="absolute right-1.5 top-1.5"
              aria-label="Copy .env configuration"
              title="Copy"
              @click="copyEnv"
            >
              <Copy class="size-4" />
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
