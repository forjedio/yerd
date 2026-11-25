<script setup lang="ts">
import { REPO } from './types'

// PR number chips plus "with @login" credits for community authors.
defineProps<{ prs?: number[]; credit?: string[] }>()
</script>

<template>
  <p v-if="prs?.length || credit?.length" class="y-credits">
    <a
      v-for="pr in prs"
      :key="pr"
      class="y-credits__pr"
      :href="`${REPO}/pull/${pr}`"
      target="_blank"
      rel="noopener"
      >#{{ pr }}</a
    >
    <span v-if="credit?.length" class="y-credits__by">
      with
      <a
        v-for="login in credit"
        :key="login"
        :href="`https://github.com/${login}`"
        target="_blank"
        rel="noopener"
        >@{{ login }}</a
      >
    </span>
  </p>
</template>
