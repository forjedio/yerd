# DMG installer background

`background.svg` is the source for the macOS `.dmg` installer window art (the
dark surface, title, drag arrow, and caption). The Finder draws the `Yerd.app`
and `Applications` icons on top, at the positions configured under
`bundle.macOS.dmg` in `../tauri.bundle-macos.conf.json` (`appPosition` /
`applicationFolderPosition`), so the arrow in the art sits between them.

The committed PNGs are what the release build consumes; regenerate them after
editing the SVG:

```sh
rsvg-convert -w 660  -h 420 background.svg -o background.png
rsvg-convert -w 1320 -h 840 background.svg -o background@2x.png
```

`create-dmg` (via Tauri) automatically uses `background@2x.png` on Retina
displays. Keep the 1× canvas at `windowSize` (660×420) and the 2× at exactly
double.

> Requires `librsvg` (`brew install librsvg`). Any SVG→PNG rasterizer works as
> long as the output dimensions match.
