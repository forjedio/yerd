import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import Layout from './Layout.vue'
import ThemedImage from './components/ThemedImage.vue'
import ShowcaseRow from './components/ShowcaseRow.vue'
import './custom.css'

// Extends the VitePress default theme with Yerd's indigo brand palette and
// hero styling (see custom.css), plus a global <ThemedImage> for light/dark
// screenshots.
export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('ThemedImage', ThemedImage)
    app.component('ShowcaseRow', ShowcaseRow)
  },
} satisfies Theme
