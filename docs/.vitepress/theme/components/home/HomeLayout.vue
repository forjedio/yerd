<script setup lang="ts">
import { useData } from 'vitepress'
import AppSection from './AppSection.vue'
import CapabilitySection from './CapabilitySection.vue'
import ClosingSection from './ClosingSection.vue'
import FootprintSection from './FootprintSection.vue'
import HeroSection from './HeroSection.vue'
import StatsBar from './StatsBar.vue'
import StepsSection from './StepsSection.vue'

// The home page layout, selected by `layout: HomeLayout` in index.md.
// Every string it renders comes from that page's frontmatter, so the marketing
// copy stays in the content file rather than in the theme.
const { frontmatter } = useData()
</script>

<template>
  <div class="y-home">
    <HeroSection v-bind="frontmatter.hero" />
    <StatsBar :stats="frontmatter.stats" />
    <FootprintSection v-bind="frontmatter.footprint" />
    <CapabilitySection v-bind="frontmatter.capabilities" />
    <StepsSection v-bind="frontmatter.steps" />
    <AppSection v-bind="frontmatter.app" />
    <ClosingSection v-bind="frontmatter.closing" />
  </div>
</template>
