<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue'
import { useData } from 'vitepress'
import ClosingSection from '../home/ClosingSection.vue'
import SectionHead from '../home/SectionHead.vue'
import StatsBar from '../home/StatsBar.vue'
import ReleaseChapter from './ReleaseChapter.vue'
import ReleaseCompare from './ReleaseCompare.vue'
import ReleaseContributors from './ReleaseContributors.vue'
import ReleaseCredits from './ReleaseCredits.vue'
import ReleaseHero from './ReleaseHero.vue'
import ReleaseNav from './ReleaseNav.vue'
import ReleaseTerminal from './ReleaseTerminal.vue'
import type { Chapter } from './types'

// A release-notes page, selected by `layout: ReleaseLayout`. Like the home
// page, every string comes from the page's frontmatter so the next release is
// a new content file rather than new components.
//
// Scroll-in reveals are opt-in: `y-reveal-on` lands on <html> only after mount,
// so the server-rendered page (and anyone without JavaScript) sees everything.
const { frontmatter } = useData()

const chapters = computed<Chapter[]>(() => frontmatter.value.chapters ?? [])

const navItems = computed(() => [
  { id: 'contributors', label: 'Contributors' },
  ...chapters.value.map((c) => ({ id: c.id, label: c.label, number: c.number })),
  { id: 'fixes', label: 'Fixes' },
  { id: 'upgrade', label: 'Upgrade' },
])

let observer: IntersectionObserver | null = null

onMounted(() => {
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
  document.documentElement.classList.add('y-reveal-on')
  observer = new IntersectionObserver(
    (entries) => {
      entries
        .filter((e) => e.isIntersecting)
        .forEach((e) => {
          e.target.classList.add('is-in')
          observer?.unobserve(e.target)
        })
    },
    { rootMargin: '0px 0px -8% 0px', threshold: 0.08 },
  )
  document.querySelectorAll('.y-release .y-reveal').forEach((el) => observer?.observe(el))
})

onUnmounted(() => {
  observer?.disconnect()
  document.documentElement.classList.remove('y-reveal-on')
})
</script>

<template>
  <div class="y-home y-release">
    <ReleaseHero v-bind="frontmatter.hero" />

    <StatsBar :stats="frontmatter.stats" />

    <ReleaseNav :items="navItems" />

    <ReleaseContributors v-bind="frontmatter.contributors" />

    <ReleaseChapter
      v-for="(chapter, i) in chapters"
      :key="chapter.id"
      :chapter="chapter"
      :flip="i % 2 === 1"
    >
      <template v-if="i === 0 && frontmatter.compare" #lead>
        <div class="y-compare-wrap y-reveal">
          <ReleaseCompare :before="frontmatter.compare.before" :after="frontmatter.compare.after" />
          <p class="y-compare-wrap__hint">{{ frontmatter.compare.hint }}</p>
        </div>
      </template>
    </ReleaseChapter>

    <section id="fixes" class="y-section y-rel-section">
      <SectionHead v-bind="frontmatter.fixes.head" />
      <ul class="y-fixes">
        <li
          v-for="(fix, i) in frontmatter.fixes.items"
          :key="fix.title"
          class="y-fix y-reveal"
          :style="{ '--y-delay': `${(i % 3) * 50}ms` }"
        >
          <span class="y-fix__tag">FIX</span>
          <h4 class="y-fix__title">{{ fix.title }}</h4>
          <p class="y-fix__text">{{ fix.text }}</p>
          <ReleaseCredits :prs="fix.prs" :credit="fix.credit" />
        </li>
      </ul>
    </section>

    <section id="upgrade" class="y-section y-rel-section y-glow y-glow--right">
      <SectionHead v-bind="frontmatter.upgrade.head" />
      <div class="y-upgrade">
        <div class="y-upgrade__notes y-reveal">
          <h3 class="y-upgrade__title">{{ frontmatter.upgrade.notesTitle }}</h3>
          <ul>
            <li v-for="note in frontmatter.upgrade.notes" :key="note">{{ note }}</li>
          </ul>
        </div>
        <ReleaseTerminal v-bind="frontmatter.upgrade.terminal" class="y-reveal" />
      </div>
    </section>

    <ClosingSection v-bind="frontmatter.closing" />
  </div>
</template>
