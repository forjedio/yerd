# DMG installer background

`background.html` is the source for the macOS `.dmg` installer window art (the
dark surface, wordmark, drag arrow, caption, and the Forjed credit). The
`Yerd.app` and `Applications` icons are placed on top by `appdmg` — headless,
no Finder/AppleScript involved (see `../scripts/build-macos-dmg.sh`) — at the
positions configured in `appdmg.json` (`contents[].x`/`y`), so the arrow in
the art sits between them.

`background.html`'s 660×420 canvas and `appdmg.json`'s `window.size` /
`contents[].x`/`y` must agree (660×420, icons at 175,205 and 485,205) — there
is no single technical source of truth for this geometry across the two
files, just this note. If you change one, change the other and regenerate the
PNGs below.

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
