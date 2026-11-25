<script setup lang="ts">
import { openLightbox } from '../../composables/lightbox'
import ReleaseCredits from './ReleaseCredits.vue'
import ReleaseShot from './ReleaseShot.vue'
import ReleaseTerminal from './ReleaseTerminal.vue'
import type { PropType } from 'vue'
import type { Chapter } from './types'

// One numbered chapter: a big index number and heading, an optional bento
// gallery, alternating feature rows (text beside one or two stacked visuals),
// and a grid of smaller changes.
defineProps({
  chapter: { type: Object as PropType<Chapter>, required: true },
  flip: { type: Boolean, default: false },
})
</script>

<template>
  <section
    :id="chapter.id"
    class="y-section y-rel-section y-chapter y-glow"
    :class="flip ? 'y-glow--left' : 'y-glow--right'"
  >
    <header class="y-chapter__head y-reveal">
      <span class="y-chapter__num" aria-hidden="true">{{ chapter.number }}</span>
      <div>
        <p class="y-head__eyebrow">{{ chapter.eyebrow }}</p>
        <h2 class="y-head__heading">{{ chapter.heading }}</h2>
        <p v-if="chapter.sub" class="y-head__sub">{{ chapter.sub }}</p>
      </div>
    </header>

    <slot name="lead" />

    <ul v-if="chapter.gallery?.length" class="y-bento">
      <li
        v-for="(shot, i) in chapter.gallery"
        :key="shot.light"
        class="y-bento__item y-reveal"
        :class="shot.span ? `is-${shot.span}` : ''"
        :style="{ '--y-delay': `${(i % 3) * 70}ms` }"
      >
        <button
          type="button"
          class="y-bento__btn"
          :aria-label="`Enlarge: ${shot.alt}`"
          @click="openLightbox(shot)"
        >
          <img class="themed-img--light" :src="shot.light" :alt="shot.alt" loading="lazy" />
          <img class="themed-img--dark" :src="shot.dark" :alt="shot.alt" loading="lazy" />
        </button>
        <span v-if="shot.caption" class="y-bento__cap">{{ shot.caption }}</span>
      </li>
    </ul>

    <div v-if="chapter.rows?.length" class="y-rows">
      <article
        v-for="(row, i) in chapter.rows"
        :key="row.title"
        class="y-row y-reveal"
        :class="{ 'is-flipped': i % 2 === 1 }"
      >
        <div class="y-row__copy">
          <p v-if="row.tag" class="y-row__tag">{{ row.tag }}</p>
          <h3 class="y-row__title">{{ row.title }}</h3>
          <p class="y-row__text">{{ row.text }}</p>
          <ul v-if="row.bullets?.length" class="y-row__bullets">
            <li v-for="b in row.bullets" :key="b">{{ b }}</li>
          </ul>
          <ReleaseCredits :prs="row.prs" :credit="row.credit" />
          <a v-if="row.link" class="y-row__link" :href="row.link">Read the guide →</a>
        </div>
        <div
          class="y-row__media"
          :class="{ 'is-stacked': row.media.length > 1 }"
        >
          <template v-for="(m, j) in row.media" :key="j">
            <ReleaseShot v-if="m.shot" :shot="m.shot" class="y-row__item" />
            <ReleaseTerminal v-else-if="m.terminal" v-bind="m.terminal" class="y-row__item" />
          </template>
        </div>
      </article>
    </div>

    <ul v-if="chapter.extras?.length" class="y-extras">
      <li
        v-for="(item, i) in chapter.extras"
        :key="item.title"
        class="y-extra y-reveal"
        :style="{ '--y-delay': `${(i % 3) * 60}ms` }"
      >
        <h4 class="y-extra__title">{{ item.title }}</h4>
        <p class="y-extra__text">{{ item.text }}</p>
        <code v-if="item.code" class="y-chip">{{ item.code }}</code>
        <ReleaseCredits :prs="item.prs" :credit="item.credit" />
      </li>
    </ul>
  </section>
</template>
