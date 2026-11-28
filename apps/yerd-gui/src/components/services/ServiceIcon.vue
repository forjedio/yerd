<script setup lang="ts">
import { computed, type Component } from "vue";
import { Database, HardDrive, Radio, Search, Server, Zap } from "lucide-vue-next";

/** The draft's icon for a service type: a database for SQL engines, a bolt for
 *  Redis, a magnifier for Meilisearch, a hard drive for S3 storage, a radio for
 *  Reverb, else a server. */
const props = defineProps<{ typeId: string }>();

const ICONS: Record<string, Component> = {
  mysql: Database,
  mariadb: Database,
  postgres: Database,
  redis: Zap,
  meilisearch: Search,
  versitygw: HardDrive,
  reverb: Radio,
};

const icon = computed(() => ICONS[props.typeId] ?? Server);
</script>

<template>
  <component :is="icon" aria-hidden="true" />
</template>
