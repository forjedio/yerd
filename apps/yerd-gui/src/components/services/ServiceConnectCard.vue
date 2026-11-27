<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ChevronsUpDown, Copy, Plug } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import { FIELD } from "@/components/services/styles";
import { CARD } from "@/components/ui/styles";
import type { ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";
import { copyWithToast } from "@/lib/toastActions";
import {
  connectKind,
  envText,
  type EnvLine,
  meilisearchEnv,
  redisEnv,
  reverbEnv,
  reverbPublic,
  rustfsEnv,
  siteDomain,
  sqlEnv,
} from "@/lib/services";
import { cn } from "@/lib/utils";

/**
 * The inspector's Connect card: the `.env` block that points a Laravel app at
 * this service, with Copy for .env. SQL engines pick the database (the
 * inspector owns that choice so a Databases row can set it), Meilisearch picks
 * a Laravel site for the Scout prefix, RustFS shows the S3 disk block, and a Reverb instance shows its server
 * and browser halves side by side (`layout="wide"`).
 */
const props = defineProps<{
  service: ServiceStatus;
  /** The running SQL engine's database names; null while unknown or stopped. */
  databases: string[] | null;
  report: StatusReport | null;
  sites: SiteEntry[];
  tld: string;
  layout: "stack" | "wide";
}>();

const database = defineModel<string>("database", { default: "" });

const kind = computed(() => connectKind(props.service));
const typeId = computed(() => props.service.type_id ?? props.service.service);
const laravelSites = computed(() => props.sites.filter((s) => s.is_laravel));
const linkedSite = computed(() => props.sites.find((s) => s.name === props.service.site) ?? null);
const dbNames = computed(() => (props.service.state === "running" ? (props.databases ?? []) : []));

const prefix = ref("");
const prefixTouched = ref(false);

watch(
  () => laravelSites.value[0]?.name,
  (first) => {
    if (!prefixTouched.value && first) prefix.value = first;
  },
  { immediate: true },
);

function setPrefix(value: string): void {
  prefixTouched.value = true;
  prefix.value = value;
}

const block = computed<EnvLine[]>(() => {
  const s = props.service;
  if (kind.value === "sql") return sqlEnv(typeId.value, s.port, database.value);
  if (kind.value === "redis") return redisEnv(s.port);
  if (kind.value === "meilisearch") return meilisearchEnv(s.port, prefix.value);
  if (kind.value === "rustfs") return rustfsEnv(s.port);
  return [];
});

const reverb = computed(() =>
  kind.value === "site"
    ? reverbEnv(props.service.port, reverbPublic(linkedSite.value, props.service.site ?? "", props.report))
    : null,
);

const title = computed(() =>
  kind.value === "site" ? `Connect ${siteDomain(props.service, props.tld, props.sites)}` : "Connect a Laravel app",
);

const dbHint = computed(() => {
  if (kind.value !== "sql" || dbNames.value.length) return "";
  return props.service.state === "running"
    ? "No databases yet."
    : `Start ${props.service.display_name} to pick a database.`;
});

async function copy(): Promise<void> {
  await copyWithToast(envText(reverb.value ?? block.value), {
    title: "Copied to clipboard",
    detail: "Paste these into your Laravel .env",
  });
}

const ENV = "rounded-md bg-sidebar font-mono text-[12px] leading-5";
const PICKER =
  "flex h-8 items-center gap-2 rounded-md border bg-background pl-2.5 pr-2 text-xs focus-within:border-brand/60";
const HEADING = "mb-1.5 text-[10px] font-medium uppercase tracking-[0.6px] text-faint";
</script>

<template>
  <section :class="cn(CARD, 'p-4')" data-testid="connect-card">
    <div class="flex items-center gap-2">
      <h3 class="flex min-w-0 flex-1 items-center gap-2 text-sm font-semibold">
        <Plug class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span class="truncate">{{ title }}</span>
      </h3>
      <Button
        v-if="layout === 'wide'"
        variant="outline"
        size="sm"
        class="h-[30px] shrink-0"
        @click="copy"
      >
        <Copy /> Copy for .env
      </Button>
    </div>

    <template v-if="kind === 'sql'">
      <label :class="cn(PICKER, 'mt-3', !dbNames.length && 'opacity-60')">
        <span class="shrink-0 text-faint">Database</span>
        <select
          v-model="database"
          class="min-w-0 flex-1 appearance-none bg-transparent font-mono text-[13px] outline-none disabled:cursor-not-allowed"
          aria-label="Database"
          :disabled="!dbNames.length"
        >
          <option v-if="!dbNames.length" value="">your_database</option>
          <option v-for="name in dbNames" :key="name" :value="name">{{ name }}</option>
        </select>
        <ChevronsUpDown class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
      </label>
      <p v-if="dbHint" class="mt-1.5 text-xs text-faint">{{ dbHint }}</p>
    </template>

    <template v-else-if="kind === 'meilisearch'">
      <label v-if="laravelSites.length" :class="cn(PICKER, 'mt-3')">
        <span class="shrink-0 text-faint">Site</span>
        <select
          :value="prefix"
          class="min-w-0 flex-1 appearance-none bg-transparent font-mono text-[13px] outline-none"
          aria-label="Laravel site"
          @change="setPrefix(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="s in laravelSites" :key="s.name" :value="s.name">{{ s.name }}</option>
        </select>
        <ChevronsUpDown class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
      </label>
      <input
        v-else
        :value="prefix"
        :class="cn(FIELD, 'mt-3 w-full')"
        placeholder="your_app"
        aria-label="Scout prefix"
        spellcheck="false"
        @input="setPrefix(($event.target as HTMLInputElement).value)"
      />
    </template>

    <div
      v-if="reverb"
      :class="cn(ENV, 'mt-3 grid grid-cols-2 divide-x divide-border')"
      data-testid="connect-env"
    >
      <div class="min-w-0 p-3">
        <p :class="HEADING">Server</p>
        <span
          v-for="line in reverb.server"
          :key="line.text"
          :class="cn('block truncate', line.strong ? 'text-foreground' : 'text-muted-foreground')"
        >{{ line.text }}</span>
      </div>
      <div class="min-w-0 p-3">
        <p :class="HEADING">Browser (Echo) — via the site's /app proxy</p>
        <span
          v-for="line in reverb.browser"
          :key="line.text"
          :class="cn('block truncate', line.strong ? 'text-foreground' : 'text-muted-foreground')"
        >{{ line.text }}</span>
      </div>
    </div>
    <pre
      v-else
      :class="cn(ENV, 'mt-3 overflow-x-auto p-3')"
      data-testid="connect-env"
    ><span
      v-for="(line, i) in block"
      :key="i"
      :class="cn('block', line.strong ? 'text-foreground' : 'text-muted-foreground')"
    >{{ line.text || " " }}</span></pre>

    <p v-if="kind === 'meilisearch'" class="mt-3 text-xs leading-relaxed text-muted-foreground">
      Install <code>laravel/scout</code> and <code>meilisearch/meilisearch-php</code>, then run
      <code>php artisan scout:sync-index-settings</code> and
      <code>php artisan scout:import "App\\Models\\YourModel"</code>. Yerd does not change project
      files or run Composer or Artisan for you.
    </p>

    <p
      v-if="kind === 'rustfs'"
      class="mt-3 text-xs leading-relaxed text-muted-foreground"
      data-testid="rustfs-note"
    >
      Install <code>league/flysystem-aws-s3-v3</code>. RustFS does not create buckets on its own:
      create <code>your_bucket</code> once with any S3 client, e.g.
      <code>AWS_ACCESS_KEY_ID=rustfsadmin AWS_SECRET_ACCESS_KEY=rustfsadmin AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://127.0.0.1:{{ service.port }} s3 mb s3://your_bucket</code>.
      Yerd does not change project files or run Composer for you.
    </p>

    <Button
      v-if="layout === 'stack'"
      variant="outline"
      class="mt-3 h-[34px] w-full"
      @click="copy"
    >
      <Copy /> Copy for .env
    </Button>
  </section>
</template>
