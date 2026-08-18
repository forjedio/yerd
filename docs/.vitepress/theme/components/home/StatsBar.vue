<script setup lang="ts">
defineProps<{ stats: { value: string; label: string }[] }>()
</script>

<template>
  <section class="y-section y-section--tight">
    <dl class="y-stats">
      <div v-for="stat in stats" :key="stat.label" class="y-stats__cell">
        <dt class="y-stats__value">{{ stat.value }}</dt>
        <dd class="y-stats__label">{{ stat.label }}</dd>
      </div>
    </dl>
  </section>
</template>
