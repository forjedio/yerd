import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import Layout from './Layout.vue'
import ThemedImage from './components/ThemedImage.vue'
import ShowcaseRow from './components/ShowcaseRow.vue'
import YouTubeEmbed from './components/YouTubeEmbed.vue'
import './custom.css'

// Extends the VitePress default theme with Yerd's indigo brand palette and
// hero styling (see custom.css), plus globals for light/dark screenshots
// (<ThemedImage>, <ShowcaseRow>) and click-to-load video embeds (<YouTubeEmbed>).
export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('ThemedImage', ThemedImage)
    app.component('ShowcaseRow', ShowcaseRow)
    app.component('YouTubeEmbed', YouTubeEmbed)
  },
} satisfies Theme
