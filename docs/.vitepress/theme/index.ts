import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import './custom.css'

// Extends the VitePress default theme with Yerd's indigo brand palette and
// hero styling (see custom.css).
export default {
  extends: DefaultTheme,
} satisfies Theme
