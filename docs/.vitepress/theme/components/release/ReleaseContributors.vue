<script setup lang="ts">
import SectionHead from '../home/SectionHead.vue'
import { REPO } from './types'

// Contributor cards: avatar, name, handle, a "first contribution" badge, and
// the pull requests each person shipped in this release.
interface Contribution {
  pr: number
  title: string
}

interface Person {
  login: string
  name: string
  first?: boolean
  role?: string
  shipped: Contribution[]
}

defineProps<{
  eyebrow: string
  heading: string
  sub?: string
  people: Person[]
  footnote?: string
}>()
</script>

<template>
  <section id="contributors" class="y-section y-rel-section y-glow y-glow--left">
    <SectionHead :eyebrow="eyebrow" :heading="heading" :sub="sub" />

    <ul class="y-people">
      <li
        v-for="(person, i) in people"
        :key="person.login"
        class="y-person y-reveal"
        :class="{ 'is-first': person.first }"
        :style="{ '--y-delay': `${(i % 4) * 60}ms` }"
      >
        <a
          class="y-person__head"
          :href="`https://github.com/${person.login}`"
          target="_blank"
          rel="noopener"
        >
          <img
            class="y-person__avatar"
            :src="`https://github.com/${person.login}.png?size=112`"
            :alt="`${person.name}'s GitHub avatar`"
            width="56"
            height="56"
            loading="lazy"
          />
          <span class="y-person__who">
            <span class="y-person__name">{{ person.name }}</span>
            <span class="y-person__login">@{{ person.login }}</span>
          </span>
        </a>
        <p v-if="person.first" class="y-person__badge">
          <span class="y-pill__dot" />FIRST CONTRIBUTION
        </p>
        <p v-else-if="person.role" class="y-person__badge is-muted">
          {{ person.role }}
        </p>
        <ul class="y-person__prs">
          <li v-for="item in person.shipped" :key="item.pr">
            <a :href="`${REPO}/pull/${item.pr}`" target="_blank" rel="noopener">
              <span class="y-person__num">#{{ item.pr }}</span>{{ item.title }}
            </a>
          </li>
        </ul>
      </li>
    </ul>

    <p v-if="footnote" class="y-people__foot">{{ footnote }}</p>
  </section>
</template>
