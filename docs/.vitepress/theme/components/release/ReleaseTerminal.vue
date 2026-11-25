<script setup lang="ts">
import { computed, type PropType } from 'vue'
import type { TerminalBlock } from './types'

// A faux terminal (or config file) window. Shell lines starting with `$ ` get a
// prompt, `#` lines read as comments, and a trailing `  # note` is dimmed.
// TOML blocks colour section headers, keys and values.
const props = defineProps({
  title: { type: String, required: true },
  kind: { type: String as PropType<TerminalBlock['kind']>, default: 'shell' },
  lines: { type: Array as PropType<string[]>, required: true },
})

interface Token {
  text: string
  cls?: string
}

function shellLine(line: string): Token[] {
  if (line.startsWith('#')) return [{ text: line, cls: 'is-comment' }]
  const isCommand = line.startsWith('$ ')
  const body = isCommand ? line.slice(2) : line
  const split = body.indexOf('  # ')
  const main = split === -1 ? body : body.slice(0, split)
  const note = split === -1 ? '' : body.slice(split)
  const tokens: Token[] = isCommand
    ? [{ text: '$ ', cls: 'is-prompt' }, { text: main, cls: 'is-cmd' }]
    : [{ text: main, cls: 'is-out' }]
  if (note) tokens.push({ text: note, cls: 'is-comment' })
  return tokens
}

function tomlLine(line: string): Token[] {
  if (line.startsWith('#')) return [{ text: line, cls: 'is-comment' }]
  if (/^\[.*\]$/.test(line)) return [{ text: line, cls: 'is-section' }]
  const eq = line.indexOf(' = ')
  if (eq === -1) return [{ text: line }]
  return [
    { text: line.slice(0, eq), cls: 'is-key' },
    { text: ' = ', cls: 'is-out' },
    { text: line.slice(eq + 3), cls: 'is-value' },
  ]
}

const rows = computed(() =>
  props.lines.map((line) => (props.kind === 'toml' ? tomlLine(line) : shellLine(line))),
)
</script>

<template>
  <figure class="y-term">
    <figcaption class="y-term__bar">
      <span class="y-term__dots" aria-hidden="true"><i /><i /><i /></span>
      <span class="y-term__title">{{ title }}</span>
    </figcaption>
    <pre class="y-term__body"><code><span
      v-for="(row, i) in rows"
      :key="i"
      class="y-term__line"
    ><span
      v-for="(token, j) in row"
      :key="j"
      :class="token.cls"
    >{{ token.text }}</span>
</span></code></pre>
  </figure>
</template>
