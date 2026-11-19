<script setup lang="ts">
import { Undo2 } from "lucide-vue-next";

import ComingSoon from "@/components/ComingSoon.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useElevation } from "@/composables/useElevation";
import { privilegeStatus } from "@/lib/elevation";

// Self-contained OS-privileges panel (CA trust, .test resolver, privileged
// ports) for the Welcome screen. The rows and actions come from useElevation,
// which the Doctor screen shares.
const { connected, report } = useDaemon();

// Emitted after any elevation/revert completes (success or failure), so a parent
// can re-run dependent checks.
const emit = defineEmits<{ elevated: [] }>();

const {
  canElevate,
  busy,
  items,
  anyFixable,
  fixAll,
  elevateOne,
  unelevateOpen,
  unelevateCopy,
  openUnelevate,
  confirmUnelevate,
} = useElevation({ onSettled: () => emit("elevated") });

type Tri = boolean | null;
function triTone(v: Tri): Tone {
  if (v === true) return "ok";
  if (v === false) return "bad";
  return "unknown";
}
</script>

<template>
  <Card>
    <CardHeader class="flex-row items-center justify-between space-y-0">
      <div class="space-y-1.5">
        <CardTitle>Environment</CardTitle>
        <CardDescription>
          OS-level trust, resolver, and privileged-port configuration.
        </CardDescription>
      </div>
      <Button
        v-if="canElevate"
        size="sm"
        :disabled="!anyFixable || busy !== null"
        @click="fixAll"
      >
        <Spinner v-if="busy === 'elevate:all'" class="size-4" />
        Fix all (elevate)
      </Button>
    </CardHeader>
    <CardContent>
      <p
        v-if="!report && connected === false"
        class="py-6 text-center text-sm text-muted-foreground"
      >
        Start the daemon to view and change OS privileges.
      </p>
      <div v-else-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
      <table v-else class="w-full text-sm">
        <thead class="sr-only">
          <tr>
            <th scope="col">Privilege</th>
            <th scope="col">Status</th>
            <th scope="col">Action</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="item in items" :key="item.key" class="border-b last:border-0">
            <td class="py-3 pr-4 font-medium">{{ item.label }}</td>
            <td class="py-3 pr-4">
              <StatusPill :tone="triTone(item.value)" :label="privilegeStatus(item)" />
            </td>
            <td class="py-3 pl-4 text-right">
              <template v-if="item.fixable">
                <Button
                  v-if="canElevate"
                  variant="outline"
                  size="sm"
                  :disabled="busy !== null"
                  @click="elevateOne(item.target)"
                >
                  <Spinner v-if="busy === `elevate:${item.target}`" class="size-4" />
                  Fix (elevate)
                </Button>
                <ComingSoon
                  v-else
                  reason="In-app elevation isn't available on this platform yet - use `yerd elevate` in a terminal for now."
                  pill
                >
                  Fix
                </ComingSoon>
              </template>
              <Button
                v-else-if="item.unelevatable && canElevate"
                variant="ghost"
                size="sm"
                :disabled="busy !== null"
                @click="openUnelevate(item.target)"
              >
                <Spinner v-if="busy === `unelevate:${item.target}`" class="size-4" />
                <Undo2 v-else class="size-4" />
                Revert
              </Button>
              <span
                v-else-if="item.note"
                class="text-xs text-muted-foreground"
              >{{ item.note }}</span>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-if="report" class="mt-3 text-xs text-muted-foreground">
        Fixes run the audited <code>yerd elevate</code> helper under an OS
        prompt; the GUI itself never runs as root.
      </p>
    </CardContent>
  </Card>

  <Modal v-model:open="unelevateOpen" :title="unelevateCopy?.title ?? 'Revert privilege'">
    <p class="text-sm text-muted-foreground">{{ unelevateCopy?.body }}</p>
    <p v-if="unelevateCopy?.note" class="mt-2 text-xs text-muted-foreground">{{ unelevateCopy.note }}</p>
    <template #footer="{ close }">
      <Button variant="ghost" @click="close">Cancel</Button>
      <Button @click="close(); confirmUnelevate()">Revert</Button>
    </template>
  </Modal>
</template>
