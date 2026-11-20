<script setup lang="ts">
import { Pencil, Plus, RotateCcw, Star, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import DomainDialog from "@/components/sites/DomainDialog.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { CARD, SECTION_LABEL } from "@/components/ui/styles";
import { useToast } from "@/composables/useToast";
import { addDomain, IpcError, removeDomain, resetDomains, setPrimaryDomain } from "@/ipc/client";
import { normalizeDomain } from "@/lib/sites";

/** The minimal shape the card needs from whatever holds the domains: a
 *  `SiteEntry` satisfies it as-is, and `ProxyInspector` maps a proxy onto it.
 *  The optional fields are the ones the daemon omits for a default target or
 *  doesn't carry for a proxy at all. */
export interface DomainsTarget {
  name: string;
  primary_domain?: string;
  domains?: string[];
  apex_shadowed_by?: string;
  is_wordpress?: boolean;
}

/**
 * The DOMAINS card of the site and proxy inspectors: every domain the site or
 * proxy answers on, tagged primary or wildcard (and alias, when `aliasTag` is
 * set), with Make primary, Edit and Remove on each row. Add domain and Edit open
 * `DomainDialog`; Reset to default asks first. The daemon has no rename, so an
 * edit adds the new domain, moves the primary across if needed, then removes the
 * old one. An edit the daemon would normalize back to the old domain (case or a
 * trailing dot) just closes, since add-then-remove would delete it. Switching to
 * another target closes an open dialog, so a staged edit can't land on it.
 */
const props = defineProps<{
  site: DomainsTarget;
  tld: string;
  /** Tag the other exact domains "alias" (as the Proxies inspector draws them). */
  aliasTag?: boolean;
}>();

const emit = defineEmits<{ domainsChanged: [] }>();

const toast = useToast();

const primary = computed(() => props.site.primary_domain ?? `${props.site.name}.${props.tld}`);

/** The DTO omits `domains` for a default apex-only target, so synthesize the apex. */
const effective = computed(() =>
  props.site.domains && props.site.domains.length ? props.site.domains : [primary.value],
);

const isWildcard = (d: string): boolean => d.startsWith("*.");
const exactCount = computed(() => effective.value.filter((d) => !isWildcard(d)).length);
const isDefault = computed(() => !props.site.primary_domain && !props.site.domains?.length);

/** Non-null while an IPC action is in flight; the per-action key (`add`,
 *  `edit:x`, `primary:x`, `remove:x`, `reset`) picks which control spins. */
const busy = ref<string | null>(null);

const dialogOpen = ref(false);
const editing = ref<string | null>(null);
const confirmReset = ref(false);

watch(
  () => props.site.name,
  () => {
    dialogOpen.value = false;
    editing.value = null;
    confirmReset.value = false;
  },
);

function openAdd(): void {
  editing.value = null;
  dialogOpen.value = true;
}

function openEdit(d: string): void {
  editing.value = d;
  dialogOpen.value = true;
}

/** Run one IPC mutation behind the shared busy flag, toasting the outcome. The
 *  parent reloads on success, and after a failure too, since a multi-step edit
 *  may have applied its first steps. Returns whether every step succeeded. */
async function run(key: string, fn: () => Promise<void>, ok: string): Promise<boolean> {
  if (busy.value !== null) return false;
  busy.value = key;
  try {
    await fn();
    toast.success(ok);
    return true;
  } catch (e) {
    toast.error("Domain change failed", (e as IpcError).message);
    return false;
  } finally {
    busy.value = null;
    emit("domainsChanged");
  }
}

async function submit(domain: string): Promise<void> {
  const old = editing.value;
  const name = props.site.name;
  if (old && normalizeDomain(domain) === normalizeDomain(old)) {
    dialogOpen.value = false;
    return;
  }
  const done = old
    ? await run(
        `edit:${old}`,
        async () => {
          await addDomain(name, domain);
          if (old === primary.value) await setPrimaryDomain(name, domain);
          await removeDomain(name, old);
        },
        `Changed ${old} to ${domain}`,
      )
    : await run("add", () => addDomain(name, domain), `Added ${domain}`);
  if (done) dialogOpen.value = false;
}

function makePrimary(d: string): void {
  void run(`primary:${d}`, () => setPrimaryDomain(props.site.name, d), `${d} is now the primary`);
}

function remove(d: string): void {
  void run(`remove:${d}`, () => removeDomain(props.site.name, d), `Removed ${d}`);
}

function resetAll(): void {
  confirmReset.value = false;
  void run("reset", () => resetDomains(props.site.name), "Reset to the default domain");
}

/** The daemon requires at least one exact domain, so the last one can't go. */
function isLastExact(d: string): boolean {
  return exactCount.value === 1 && !isWildcard(d);
}

function rowBusy(d: string): boolean {
  return [`edit:${d}`, `primary:${d}`, `remove:${d}`].includes(busy.value ?? "");
}
</script>

<template>
  <section>
    <div class="mb-2 flex items-center justify-between px-1">
      <h3 :class="SECTION_LABEL">Domains</h3>
      <button
        v-if="!isDefault"
        type="button"
        class="flex items-center gap-1 text-[11px] font-medium text-faint transition-colors hover:text-foreground disabled:opacity-50"
        :disabled="busy !== null"
        title="Clear all custom domains and return to the default"
        @click="confirmReset = true"
      >
        <Spinner v-if="busy === 'reset'" class="size-3" />
        <RotateCcw v-else class="size-3" aria-hidden="true" />
        Reset to default
      </button>
    </div>
    <div :class="['divide-y overflow-hidden', CARD]" data-testid="domains-card">
      <div
        v-for="d in effective"
        :key="d"
        class="group flex h-11 items-center gap-3 px-4"
        data-testid="domain-row"
      >
        <span class="min-w-0 flex-1 truncate font-mono text-[13px]">
          <template v-if="isWildcard(d)"><span class="text-faint">*</span>{{ d.slice(1) }}</template>
          <template v-else>{{ d }}</template>
        </span>
        <span v-if="d === primary" class="shrink-0 text-[11px] font-medium text-brand">primary</span>
        <span v-else-if="isWildcard(d)" class="shrink-0 text-[11px] text-faint">wildcard</span>
        <span v-else-if="aliasTag" class="shrink-0 text-[11px] text-faint">alias</span>
        <div
          class="flex shrink-0 items-center gap-0.5 opacity-60 transition-opacity focus-within:opacity-100 group-hover:opacity-100"
        >
          <Spinner v-if="rowBusy(d)" class="mx-1 size-3.5" />
          <Button
            v-if="d !== primary && !isWildcard(d)"
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :disabled="busy !== null"
            :aria-label="`Make ${d} primary`"
            :title="
              site.is_wordpress
                ? 'Make primary (also updates the WordPress site URL)'
                : 'Make primary'
            "
            @click="makePrimary(d)"
          >
            <Star class="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :disabled="busy !== null"
            :aria-label="`Edit ${d}`"
            title="Edit domain"
            @click="openEdit(d)"
          >
            <Pencil class="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 hover:text-destructive"
            :disabled="busy !== null || isLastExact(d)"
            :aria-label="`Remove ${d}`"
            :title="isLastExact(d) ? 'A site must keep at least one exact domain' : 'Remove domain'"
            @click="remove(d)"
          >
            <Trash2 class="size-3.5" />
          </Button>
        </div>
      </div>
      <p
        v-if="site.apex_shadowed_by"
        class="px-4 py-2.5 text-xs text-warning"
        data-testid="domains-shadowed"
      >
        {{ site.name }}.{{ tld }} is served by “{{ site.apex_shadowed_by }}”. Give this one a
        different primary domain to route it separately.
      </p>
      <button
        type="button"
        class="flex h-11 w-full items-center gap-2 px-4 text-left text-[13px] font-medium text-brand transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring disabled:opacity-50"
        :disabled="busy !== null"
        @click="openAdd"
      >
        <Plus class="size-3.5" aria-hidden="true" /> Add domain
      </button>
    </div>

    <DomainDialog
      v-model:open="dialogOpen"
      :name="site.name"
      :tld="tld"
      :editing="editing"
      :editing-primary="editing === primary"
      :is-wordpress="site.is_wordpress"
      :busy="busy === 'add' || busy?.startsWith('edit:')"
      @submit="submit"
    />

    <ConfirmDialog
      v-model:open="confirmReset"
      title="Reset domains?"
      confirm-label="Reset to default"
      tone="danger"
      :icon="RotateCcw"
      @confirm="resetAll"
    >
      Every custom domain is removed and {{ site.name }}.{{ tld }} becomes the only domain again.
    </ConfirmDialog>
  </section>
</template>
