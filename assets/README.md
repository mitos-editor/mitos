# Mitos icons

Edit `Mitos.icon` in Icon Composer to change the colours and glass effects.

`icons/` contains three 1024px PNGs:

- `mitos-light.png`: white logo on the gradient.
- `mitos-dark.png`: coloured logo on a dark background.
- `mitos-site-light.png`: coloured logo on white with a faint gradient tint.

The light and dark icons also have `.ico` and `.icns` versions for packaging.
They do not switch appearance automatically. The release workflows do not yet
use them.

The site uses the site-light and dark icons. Its SVG favicon switches with the
browser's colour preference; the PNG fallback uses site-light.

## Export

Save `Mitos.icon`, then run on macOS:

```sh
python3 assets/icon-composer/export.py
```

Requires Icon Composer 27, ImageMagick, and macOS `iconutil`. Use
`--ictool /path/to/ictool` if Icon Composer is installed inside Xcode.
Use `--skip-render` to rebuild the favicon and package formats from existing PNGs.

The script creates the site-light version during export, preserving the logo's
glass effects and blending the background gradient over white at 7% opacity.

## Change the artwork

Edit the SVGs in `Mitos.icon/Assets`: `01-background-gradient.svg` defines the
colours and `02-logo-white.svg` defines the shape. Then run:

```sh
python3 assets/icon-composer/generate.py
```

This requires librsvg's `rsvg-convert`. It rebuilds `03-logo-gradient.png` from
those SVGs. Reopen the document in Composer, check each appearance,
save, and export again. Update `book/brand.css` if the palette changes.
