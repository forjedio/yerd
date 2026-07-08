<script setup lang="ts">
import { Plus, Star, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import { addDomain, IpcError, removeDomain, resetDomains, setPrimaryDomain } from "@/ipc/client";
import { isUnderTld, validateDomainShape } from "@/lib/domainValidation";
import type { SiteEntry } from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  site: SiteEntry;
  tld: string;
}>();
const emit = defineEmits<{ "update:open": [boolean]; changed: [] }>();

const toast = useToast();

/** The primary FQDN: the site's `primary_domain` when customised, else the
 *  synthesized default apex (the daemon omits `primary_domain` for a default
 *  site). */
const primary = computed(() => props.site.primary_domain ?? `${props.site.name}.${props.tld}`);

/** The full effective domain set as FQDNs, in daemon (apex-first) order. The DTO
 *  omits `domains` for a default apex-only site, so synthesize the single apex. */
const effective = computed(() =>
  props.site.domains && props.site.domains.length ? props.site.domains : [primary.value],
);

const isWildcard = (d: string): boolean => d.startsWith("*.");
const exactCount = computed(() => effective.value.filter((d) => !isWildcard(d)).length);
const isDefault = computed(() => !props.site.primary_domain && !props.site.domains?.length);

/** Non-null while an IPC action is in flight; the value is a per-action key
 *  (`add:x` / `primary:x` / `remove:x` / `reset`) so exactly one row/button shows
 *  a spinner while every control is disabled. */
const busy = ref<string | null>(null);

const newDomain = ref("");
const shapeError = computed(() =>
  newDomain.value.trim() === "" ? null : validateDomainShape(newDomain.value.trim()),
);
/** A soft, non-blocking notice when the input isn't under the site's TLD. The
 *  daemon is authoritative (it rejects with `NotUnderTld`), so this never gates
 *  the Add button - only `shapeError` does. */
const tldHint = computed(() => {
  const v = newDomain.value.trim();
  return v !== "" && shapeError.value === null && !isUnderTld(v, props.tld)
    ? `Expected to end in .${props.tld} - the daemon may reject this.`
    : null;
});
const canAdd = computed(
  () => newDomain.value.trim() !== "" && shapeError.value === null && busy.value === null,
);

// Clear the add field when the modal is pointed at a different site (the parent
// keeps a single instance and swaps the `site` prop rather than remounting).
watch(
  () => props.site.name,
  () => {
    newDomain.value = "";
  },
);

/** Run one IPC mutation behind the shared busy flag, toasting the outcome and
 *  notifying the parent to reload on success. Daemon-authoritative rejections
 *  (bad shape the client let through, wrong TLD, a domain already claimed
 *  elsewhere) arrive as a thrown `IpcError` and are surfaced here. */
async function run(key: string, fn: () => Promise<void>, ok: string): Promise<void> {
  if (busy.value !== null) return;
  busy.value = key;
  try {
    await fn();
    toast.success(ok);
    emit("changed");
  } catch (e) {
    toast.error("Domain change failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

function addAlias(): void {
  const d = newDomain.value.trim();
  if (!canAdd.value) return;
  void run(
    `add:${d}`,
    async () => {
      await addDomain(props.site.name, d);
      newDomain.value = "";
    },
    `Added ${d}`,
  );
}

function makePrimary(d: string): void {
  void run(`primary:${d}`, () => setPrimaryDomain(props.site.name, d), `${d} is now the primary`);
}

function remove(d: string): void {
  void run(`remove:${d}`, () => removeDomain(props.site.name, d), `Removed ${d}`);
}

function resetAll(): void {
  void run("reset", () => resetDomains(props.site.name), "Reset to the default domain");
}

/** The sole remaining exact (non-wildcard) domain can't be removed - the daemon
 *  requires ≥1 exact, so the button is disabled to avoid a doomed request. */
function removeDisabled(d: string): boolean {
  return busy.value !== null || (exactCount.value === 1 && !isWildcard(d));
}
</script>

<template>
  <Modal
    :open="open"
    :title="`Domains — ${site.name}`"
    :dismissible="busy === null"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div class="space-y-4">
      <p
        v-if="site.apex_shadowed_by"
        class="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-400"
      >
        {{ site.name }}.{{ tld }} is currently served by "{{ site.apex_shadowed_by }}". Give this
        site a different primary domain to route it separately.
      </p>

      <ul class="space-y-1.5">
        <li
          v-for="d in effective"
          :key="d"
          class="flex items-center gap-2 rounded-md border bg-card px-3 py-2"
        >
          <span class="truncate font-mono text-sm">{{ d }}</span>
          <Badge v-if="d === primary" variant="secondary" class="shrink-0">primary</Badge>

          <div class="ml-auto flex shrink-0 items-center gap-1">
            <Spinner
              v-if="busy === `primary:${d}` || busy === `remove:${d}`"
              class="size-4"
            />
            <Button
              v-if="d !== primary && !isWildcard(d)"
              variant="ghost"
              size="sm"
              :disabled="busy !== null"
              title="Make this the primary (canonical) domain"
              @click="makePrimary(d)"
            >
              <Star class="size-3.5" /> Make primary
            </Button>
            <Button
              variant="ghost"
              size="icon"
              :disabled="removeDisabled(d)"
              :title="
                exactCount === 1 && !isWildcard(d)
                  ? 'A site must keep at least one exact domain'
                  : `Remove ${d}`
              "
              :aria-label="`Remove ${d}`"
              @click="remove(d)"
            >
              <Trash2 class="size-4" />
            </Button>
          </div>
        </li>
      </ul>

      <div>
        <label for="add-domain" class="text-sm font-medium">Add a domain</label>
        <div class="mt-2 flex gap-2">
          <Input
            id="add-domain"
            v-model="newDomain"
            :placeholder="`api.${site.name}.${tld}`"
            class="flex-1"
            @keydown.enter="addAlias"
          />
          <Button :disabled="!canAdd" @click="addAlias">
            <Spinner v-if="busy?.startsWith('add:')" class="size-4" />
            <Plus v-else class="size-4" /> Add
          </Button>
        </div>
        <p v-if="shapeError" class="mt-1 text-xs text-destructive">{{ shapeError }}</p>
        <p v-else-if="tldHint" class="mt-1 text-xs text-amber-600 dark:text-amber-500">
          {{ tldHint }}
        </p>
        <p v-else class="mt-1 text-xs text-muted-foreground">
          An exact domain (<code class="font-mono">api.{{ site.name }}.{{ tld }}</code>) or a
          single-label wildcard (<code class="font-mono">*.{{ site.name }}.{{ tld }}</code>).
        </p>
      </div>

      <p v-if="site.is_wordpress" class="text-xs text-muted-foreground">
        This is a WordPress site — changing the primary also updates its site URL.
      </p>
    </div>

    <template #footer="{ close }">
      <Button
        variant="outline"
        class="mr-auto"
        :disabled="isDefault || busy !== null"
        title="Clear all custom domains and return to the default"
        @click="resetAll"
      >
        <Spinner v-if="busy === 'reset'" class="size-4" /> Reset to default
      </Button>
      <Button variant="ghost" :disabled="busy !== null" @click="close">Close</Button>
    </template>
  </Modal>
</template>
