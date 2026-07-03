# DMG installer background

`background.html` is the source for the macOS `.dmg` installer window art (the
dark surface, wordmark, drag arrow, caption, and the Forjed credit). The
Finder draws the `Yerd.app` and `Applications` icons on top, at the positions
configured under `bundle.macOS.dmg` in `../tauri.bundle-macos.conf.json`
(`appPosition` / `applicationFolderPosition`), so the arrow in the art sits
between them.

The wordmarks ("YERD" / "FORJED") use Forjed's "Outage Cut" brand display
face (loaded from `../../src/assets/fonts/OutageCut.ttf`), so the art is
rendered with a real browser rather than `rsvg-convert` — `librsvg` can't
resolve a font that isn't installed system-wide.

The committed PNGs are what the release build consumes; regenerate them after
editing the HTML by taking two screenshots of the `.canvas` element with a
Chromium-based browser (the HTML sets `zoom: 2`, so a screenshot of the page
*is* the 2x asset):

```
window size 1320x840 → screenshot → background@2x.png
sips -z 420 660 background@2x.png -o background.png   # exact 50% downsample
```

Keep the 1x PNG at `windowSize` (660×420) and the 2x PNG at exactly double —
`sips -z <height> <width>` guarantees that relationship.
