<script setup lang="ts">
import { Mail } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

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
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  openInBrowser,
  setMailEnabled,
  setMailPort,
  showMailsWindow,
} from "@/ipc/client";

const DOCS_URL = "https://yerd.dev/guide/features";

const toast = useToast();
const { report, refresh } = useDaemon();

// Live mail status comes from the shared 4s status poll (no extra loop).
const mail = computed(() => report.value?.mail ?? null);

// Local editable copies of the config values, seeded from the live status and
// re-seeded whenever it changes (unless the user is mid-edit).
const portInput = ref("");
const enabled = ref(false);
const busy = ref(false);
let dirtyPort = false;

watch(
  mail,
  (m) => {
    if (!m) return;
    enabled.value = m.enabled;
    if (!dirtyPort) portInput.value = String(m.port);
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
  return m.listening ? "Running" : "Enabled — port unavailable";
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

async function onSavePort(): Promise<void> {
  const port = Number(portInput.value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    toast.error("Invalid port", "Enter a number between 1 and 65535.");
    return;
  }
  busy.value = true;
  try {
    await setMailPort(port);
    dirtyPort = false;
    toast.success("Mail port saved", "Takes effect after the daemon restarts.");
    await refresh();
  } catch (e) {
    toast.error("Couldn't save the mail port", (e as IpcError).message);
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
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Mail"
      subtitle="Capture and inspect emails your apps send during development"
    />

    <div class="flex-1 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Mail class="size-4" /> Mail Server
          </CardTitle>
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
              <p class="text-sm font-medium">Mail Server Status</p>
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

          <!-- Port -->
          <div>
            <label class="text-sm font-medium" for="mail-port">Mail Server Port</label>
            <p class="mb-2 text-xs text-muted-foreground">
              The port number the mail server will listen on.
            </p>
            <div class="flex items-center gap-2">
              <Input
                id="mail-port"
                v-model="portInput"
                class="max-w-32"
                placeholder="2525"
                @input="dirtyPort = true"
              />
              <Button variant="outline" size="sm" :disabled="busy" @click="onSavePort">
                <Spinner v-if="busy" class="size-4" /> Save
              </Button>
            </div>
          </div>

          <!-- Actions -->
          <div class="flex items-center justify-between border-t pt-4">
            <Button variant="ghost" size="sm" @click="openInBrowser(DOCS_URL)">
              Learn how to use Yerd's mailserver
            </Button>
            <Button @click="onShowMails">Show Mails</Button>
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
