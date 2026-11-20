<script setup lang="ts">
import { RefreshCw } from "lucide-vue-next";
import type { UnwrapNestedRefs } from "vue";

import Button from "@/components/ui/Button.vue";
import Combobox from "@/components/ui/Combobox.vue";
import Input from "@/components/ui/Input.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import type { WordPressStarter } from "@/composables/useWordPressStarter";
import type { WordPressDbEngine } from "@/ipc/types";
import { WORDPRESS_LOCALES } from "@/lib/wordpressLocales";

/**
 * Step 2 of New site for WordPress: the site title, the admin account, the core
 * version and locale, and the database the create job provisions.
 */
const props = defineProps<{ starter: UnwrapNestedRefs<WordPressStarter> }>();

const ENGINE_OPTIONS = [
  { value: "mysql", label: "MySQL" },
  { value: "mariadb", label: "MariaDB" },
];

function setEngine(v: string): void {
  props.starter.selectDbEngine(v as WordPressDbEngine);
}

function setLocale(v: string | null): void {
  props.starter.form.locale = v ?? "";
}

function setDbName(v: string): void {
  props.starter.form.dbName = v;
  props.starter.touchDbName();
}
</script>

<template>
  <div class="space-y-4">
    <div>
      <label class="text-[13px] font-semibold" for="ns-wp-title">Site title</label>
      <Input id="ns-wp-title" v-model="starter.form.siteTitle" placeholder="My Blog" class="mt-2" />
    </div>

    <div class="grid grid-cols-2 gap-4">
      <div>
        <label class="text-[13px] font-semibold" for="ns-wp-admin-user">Admin username</label>
        <Input id="ns-wp-admin-user" v-model="starter.form.adminUser" class="mt-2" />
      </div>
      <div>
        <label class="text-[13px] font-semibold" for="ns-wp-admin-email">Admin email</label>
        <Input
          id="ns-wp-admin-email"
          v-model="starter.form.adminEmail"
          type="email"
          class="mt-2"
        />
        <p
          v-if="starter.form.adminEmail && !starter.emailValid"
          class="mt-1 text-xs text-destructive"
        >
          Enter a valid email address.
        </p>
      </div>
    </div>

    <div>
      <label class="text-[13px] font-semibold" for="ns-wp-admin-password">Admin password</label>
      <div class="mt-2 flex gap-2">
        <Input
          id="ns-wp-admin-password"
          v-model="starter.form.adminPassword"
          type="text"
          placeholder="At least 8 characters"
          class="font-mono"
        />
        <Button variant="outline" @click="starter.regeneratePassword()">
          <RefreshCw class="size-4" /> Generate
        </Button>
      </div>
    </div>

    <div class="grid grid-cols-2 gap-4">
      <div>
        <label class="text-[13px] font-semibold" for="ns-wp-version">Core version</label>
        <Select
          id="ns-wp-version"
          :model-value="starter.form.coreVersion"
          :options="starter.versionOptions"
          :disabled="starter.wordpressVersionsLoading"
          class="mt-2 w-full"
          aria-label="Core version"
          @update:model-value="(v: string) => (starter.form.coreVersion = v)"
        />
      </div>
      <div>
        <span class="text-[13px] font-semibold">Locale</span>
        <div class="mt-2">
          <Combobox
            :model-value="starter.form.locale"
            :options="WORDPRESS_LOCALES"
            placeholder="en_US"
            search-placeholder="Search locales…"
            empty-text="No matching locale."
            aria-label="Locale"
            @update:model-value="setLocale"
          />
        </div>
      </div>
    </div>

    <div class="rounded-[10px] border bg-background px-3.5 py-3">
      <div class="flex items-center justify-between gap-4">
        <p class="text-[13px] font-semibold">Database engine</p>
        <SegmentedControl
          :model-value="starter.form.dbEngine"
          :options="ENGINE_OPTIONS"
          variant="plain"
          aria-label="Database engine"
          @update:model-value="setEngine"
        />
      </div>
      <p
        v-if="!starter.servicesLoading && starter.engineStateText"
        class="mt-2 text-xs text-muted-foreground"
      >
        {{ starter.engineStateText }}
      </p>
      <div class="mt-3 flex gap-4">
        <div class="flex-1">
          <label class="text-[13px] font-semibold" for="ns-wp-dbname">Database name</label>
          <Input
            id="ns-wp-dbname"
            :model-value="starter.form.dbName"
            class="mt-2 font-mono"
            @update:model-value="setDbName"
          />
          <p
            v-if="starter.form.dbName && !starter.dbNameValid"
            class="mt-1 text-xs text-destructive"
          >
            Use letters, numbers and underscores, starting with a letter or underscore.
          </p>
        </div>
        <div class="w-32 shrink-0">
          <label class="text-[13px] font-semibold" for="ns-wp-table-prefix">Table prefix</label>
          <Input id="ns-wp-table-prefix" v-model="starter.form.tablePrefix" class="mt-2 font-mono" />
        </div>
      </div>
      <p class="mt-3 text-[11px] text-faint">
        Only MySQL and MariaDB are supported for WordPress core. Yerd provisions the database as
        part of creating this site.
      </p>
    </div>
  </div>
</template>
