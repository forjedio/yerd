<script setup lang="ts">
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import { DATABASE_LABELS, KIT_OPTIONS, type LaravelStarter } from "@/composables/useLaravelStarter";
import type { AuthProvider, Database, JsRuntime, Testing } from "@/ipc/types";
import { cn } from "@/lib/utils";
import type { UnwrapNestedRefs } from "vue";

/**
 * Step 2 of New site for Laravel: the starter kit tiles, the option rows and the
 * "What Yerd will do" strip previewing the `laravel new` command.
 */
const props = defineProps<{ starter: UnwrapNestedRefs<LaravelStarter> }>();

const AUTH_OPTIONS = [
  { value: "laravel", label: "Built-in" },
  { value: "work_os", label: "WorkOS" },
];
const TESTING_OPTIONS = [
  { value: "pest", label: "Pest" },
  { value: "php_unit", label: "PHPUnit" },
];
const JS_OPTIONS = [
  { value: "npm", label: "npm" },
  { value: "bun", label: "Bun" },
  { value: "pnpm", label: "pnpm" },
  { value: "skip", label: "Later" },
];
const DATABASE_OPTIONS = (Object.keys(DATABASE_LABELS) as Database[]).map((d) => ({
  value: d,
  label: DATABASE_LABELS[d],
}));

function setAuth(v: string): void {
  props.starter.form.auth = v as AuthProvider;
}
function setTesting(v: string): void {
  props.starter.form.testing = v as Testing;
}
function setJs(v: string): void {
  props.starter.form.js = v as JsRuntime;
}
function setDatabase(v: string): void {
  props.starter.form.database = v as Database;
}

const ROW = "flex min-h-[52px] items-center justify-between gap-4 px-3.5 py-2";
</script>

<template>
  <div>
    <h3 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Starter kit</h3>
    <div class="mt-2.5 grid grid-cols-3 gap-2.5">
      <button
        v-for="k in KIT_OPTIONS"
        :key="k.value"
        type="button"
        :data-kit="k.value"
        :aria-pressed="starter.kitChoice === k.value"
        :class="
          cn(
            'flex h-12 min-w-0 flex-col justify-center rounded-[10px] border px-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            k.value === 'community' && starter.kitChoice !== k.value && 'border-dashed',
            starter.kitChoice === k.value
              ? 'border-brand bg-brand/10 ring-1 ring-brand'
              : 'bg-background hover:border-brand/40',
          )
        "
        @click="starter.kitChoice = k.value"
      >
        <span class="text-[13px] font-semibold">{{ k.label }}</span>
        <span
          :class="cn('truncate text-[11px] text-muted-foreground', k.value === 'community' && 'font-mono')"
        >
          {{ k.hint }}
        </span>
      </button>
    </div>

    <div v-if="starter.kitChoice === 'community'" class="mt-3">
      <label class="text-[13px] font-semibold" for="ns-pkg">Community package</label>
      <Input
        id="ns-pkg"
        v-model="starter.form.communityPackage"
        placeholder="vendor/starter-kit"
        class="mt-2 font-mono"
        aria-label="Community package"
      />
    </div>

    <div class="mt-4 divide-y rounded-[10px] border bg-background">
      <div v-if="starter.isJsKit || starter.isLivewire" :class="ROW">
        <p class="text-[13px] font-semibold">Authentication</p>
        <SegmentedControl
          :model-value="starter.form.auth"
          :options="AUTH_OPTIONS"
          variant="plain"
          aria-label="Authentication provider"
          @update:model-value="setAuth"
        />
      </div>
      <div v-if="starter.hasAuthKit" :class="ROW">
        <p class="text-[13px] font-semibold">Team support</p>
        <Switch v-model="starter.form.teams" aria-label="Team support" />
      </div>
      <div v-if="starter.isLivewire" :class="ROW">
        <p class="text-[13px] font-semibold">Standalone Livewire class components</p>
        <Switch
          v-model="starter.form.livewireClassComponents"
          aria-label="Livewire class components"
        />
      </div>
      <div :class="ROW">
        <p class="text-[13px] font-semibold">Testing</p>
        <SegmentedControl
          :model-value="starter.form.testing"
          :options="TESTING_OPTIONS"
          variant="plain"
          aria-label="Testing framework"
          @update:model-value="setTesting"
        />
      </div>
      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Database</p>
          <p v-if="starter.form.database !== 'sqlite'" class="text-[11px] text-faint">
            The driver is written to <code class="font-mono">.env</code>; provision the database
            yourself.
          </p>
        </div>
        <Select
          :model-value="starter.form.database"
          :options="DATABASE_OPTIONS"
          class="w-40 shrink-0"
          aria-label="Database"
          @update:model-value="setDatabase"
        />
      </div>
      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Frontend dependencies</p>
          <p v-if="starter.autoInstallLabel" class="text-[11px] text-faint">
            {{ starter.autoInstallLabel }} will be installed automatically.
          </p>
        </div>
        <SegmentedControl
          :model-value="starter.form.js"
          :options="JS_OPTIONS"
          variant="plain"
          aria-label="Frontend dependencies"
          @update:model-value="setJs"
        />
      </div>
      <div v-if="starter.showNodePicker" :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Node version</p>
          <p class="text-[11px] text-faint">Builds with it and saves it as the site's Node version.</p>
        </div>
        <Select
          v-model="starter.form.node"
          :options="starter.nodeOptions"
          class="w-48 shrink-0"
          aria-label="Node version"
        />
      </div>
      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Initialise git</p>
          <p class="text-[11px] text-faint">Run <code class="font-mono">git init</code> in the new project.</p>
        </div>
        <Switch v-model="starter.form.git" aria-label="Initialise git" />
      </div>
      <div :class="ROW">
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">Laravel Boost</p>
          <p class="text-[11px] text-faint">Install Boost for AI-assisted coding.</p>
        </div>
        <Switch v-model="starter.form.boost" aria-label="Laravel Boost" />
      </div>
    </div>

    <div class="mt-4 rounded-[10px] border bg-background px-3.5 py-3" data-testid="ns-command">
      <p class="text-[10px] font-semibold uppercase tracking-[0.6px] text-faint">What Yerd will do</p>
      <p class="mt-1.5 break-words font-mono text-[12.5px]">{{ starter.command }}</p>
      <p v-if="starter.note" class="mt-1.5 text-xs text-muted-foreground">{{ starter.note }}</p>
    </div>
  </div>
</template>
