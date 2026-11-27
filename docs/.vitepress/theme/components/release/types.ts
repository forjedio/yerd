/** A light/dark screenshot pair. Same shape as the home page's `Shot`. */
export interface Shot {
  light: string
  dark: string
  alt: string
}

/** A screenshot with an optional mono caption, as used in galleries. */
export interface CaptionedShot extends Shot {
  caption?: string
  span?: 'wide' | 'tall'
}

/** A faux terminal or file window. `kind: toml` highlights a config file. */
export interface TerminalBlock {
  title: string
  kind?: 'shell' | 'toml'
  lines: string[]
}

/** One visual beside a feature: a screenshot or a terminal window. */
export interface Media {
  shot?: Shot
  terminal?: TerminalBlock
}

/** Pull request numbers and community authors credited on an item. */
export interface Credits {
  prs?: number[]
  credit?: string[]
}

/** A headline feature, rendered as an alternating text + media row. */
export interface FeatureRow extends Credits {
  tag?: string
  title: string
  text: string
  bullets?: string[]
  link?: string
  media: Media[]
}

/** A smaller change, rendered as a compact card. */
export interface ExtraItem extends Credits {
  title: string
  text: string
  code?: string
}

/** A themed group of features with its own anchor in the chapter nav. */
export interface Chapter {
  id: string
  number: string
  label: string
  eyebrow: string
  heading: string
  sub?: string
  gallery?: CaptionedShot[]
  rows?: FeatureRow[]
  extras?: ExtraItem[]
}

export const REPO = 'https://github.com/forjedio/yerd'
