import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import Layout from './Layout.vue'
import ThemedImage from './components/ThemedImage.vue'
import YouTubeEmbed from './components/YouTubeEmbed.vue'
import HomeLayout from './components/home/HomeLayout.vue'
import './custom.css'

// Extends the VitePress default theme with Yerd's palette and typography (see
// styles/), the bespoke home page (`layout: HomeLayout` in index.md), and the
// globals markdown pages use: light/dark screenshots (<ThemedImage>) and
// click-to-load video embeds (<YouTubeEmbed>).
export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('HomeLayout', HomeLayout)
    app.component('ThemedImage', ThemedImage)
    app.component('YouTubeEmbed', YouTubeEmbed)
  },
} satisfies Theme
