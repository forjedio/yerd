<script setup lang="ts">
import { ref } from 'vue'

// The hero's "your first command" field: a read-only terminal line with a copy
// button. Falls back silently when the Clipboard API is unavailable (insecure
// origins), leaving the text selectable by hand.
const props = defineProps<{ label: string; command: string }>()

const copied = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

async function copy(): Promise<void> {
  try {
    await navigator.clipboard.writeText(props.command)
  } catch {
    return
  }
  copied.value = true
  clearTimeout(timer)
  timer = setTimeout(() => (copied.value = false), 1600)
}
</script>

<template>
  <div class="y-cmd">
    <p class="y-cmd__label">{{ label }}</p>
    <div class="y-cmd__bar">
      <code class="y-cmd__text"
        ><span class="y-cmd__prompt">$</span> {{ command
        }}<span class="y-cmd__caret" aria-hidden="true"></span
      ></code>
      <button
        class="y-cmd__copy"
        type="button"
        :aria-label="copied ? 'Copied' : 'Copy command'"
        @click="copy"
      >
        <svg
          v-if="!copied"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.7"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="9" y="9" width="12" height="12" rx="2" />
          <path d="M5 15V5a2 2 0 0 1 2-2h10" />
        </svg>
        <svg
          v-else
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M4 12.5l5 5 11-11" />
        </svg>
      </button>
    </div>
  </div>
</template>
