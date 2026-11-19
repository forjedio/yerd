<script setup lang="ts">
import { Copy } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import Switch from "@/components/ui/Switch.vue";
import type { MailStatus } from "@/ipc/types";

/**
 * Mail setup, opened from the Mail toolbar: the capture switch, the SMTP
 * address with a link to Settings ▸ Application Ports where the port lives,
 * and the Laravel `.env` block with its From name and address inputs.
 */
defineProps<{
  open: boolean;
  capture: MailStatus | null;
  /** The Laravel `.env` lines to show and copy. */
  envText: string;
  /** A capture toggle is in flight. */
  toggling: boolean;
}>();

const fromName = defineModel<string>("fromName", { required: true });
const fromAddress = defineModel<string>("fromAddress", { required: true });

const emit = defineEmits<{
  "update:open": [boolean];
  toggle: [enabled: boolean];
  copy: [];
  changePort: [];
}>();
</script>

<template>
  <DialogFrame :open="open" title="Mail setup" width="lg" @update:open="emit('update:open', $event)">
    <div class="flex items-start gap-4">
      <div class="min-w-0 flex-1">
        <p class="text-sm font-semibold">Capture outgoing mail</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Runs an SMTP server alongside Yerd. A change applies when Yerd restarts.
        </p>
      </div>
      <Switch
        class="mt-0.5"
        :model-value="capture?.enabled ?? false"
        :disabled="toggling || !capture"
        aria-label="Capture outgoing mail"
        @update:model-value="emit('toggle', $event)"
      />
    </div>

    <div class="mt-4 flex items-end gap-4">
      <div class="min-w-0 flex-1">
        <p class="text-sm font-semibold">SMTP server</p>
        <p class="mt-0.5 text-xs text-muted-foreground" data-testid="smtp-address">
          <span class="font-mono text-foreground">127.0.0.1:{{ capture?.port ?? 2525 }}</span>
          · no auth, no encryption
        </p>
      </div>
      <button
        type="button"
        class="shrink-0 text-xs font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        @click="emit('changePort')"
      >
        Change port
      </button>
    </div>

    <div class="mt-5 border-t pt-4">
      <p class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Laravel .env</p>
      <div class="mt-3 grid gap-3 sm:grid-cols-2">
        <div>
          <label class="text-xs font-medium" for="mail-from-name">From name</label>
          <Input id="mail-from-name" v-model="fromName" class="mt-1.5" placeholder="${APP_NAME}" />
          <p class="mt-1 text-[11px] text-muted-foreground">Defaults to your app's <code>APP_NAME</code>.</p>
        </div>
        <div>
          <label class="text-xs font-medium" for="mail-from-address">From address</label>
          <Input id="mail-from-address" v-model="fromAddress" class="mt-1.5" placeholder="hello@example.com" />
          <p class="mt-1 text-[11px] text-muted-foreground">Capture accepts any address - nothing is actually delivered.</p>
        </div>
      </div>
      <div class="relative mt-4">
        <pre
          class="overflow-x-auto rounded-lg border bg-sidebar p-4 pr-12 font-mono text-xs leading-5 text-foreground"
          data-testid="env-block"
        >{{ envText }}</pre>
        <Button
          variant="outline"
          size="icon"
          class="absolute right-2.5 top-2.5 size-7"
          aria-label="Copy .env configuration"
          title="Copy"
          @click="emit('copy')"
        >
          <Copy class="!size-3.5" />
        </Button>
      </div>
    </div>

    <template #footer>
      <Button @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
