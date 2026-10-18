# Image previews

Mitos can preview PNG, JPEG, GIF, and WebP files directly in the terminal.
Previews are read-only and fit the available pane. A caption shows the image
format, dimensions in pixels, and file size.

## Open an image

Open an image from your shell:

```sh
ms screenshot.png
```

Inside Mitos, use `:open screenshot.png`, the file picker (`Space f`), or the
file explorer (`Space e`). The [picker](./pickers.md) can also show an image
in its preview pane before you open it.

## Terminal rendering

Mitos detects the terminal's image capabilities automatically and uses Kitty
graphics, Sixel, or iTerm2 inline images. If no supported protocol is detected,
the editor and picker show a binary-file placeholder instead of an image.

Images resize to fit when the pane size changes. GIF and WebP previews show
a still image; animation playback is not supported.

## Preview limitations

Image previews do not provide image editing. Unsupported formats, files that
cannot be decoded, or images that exceed the decoder's limits show a binary-file
placeholder instead.

The decoder limits image width and height to 32,768 pixels and its allocation
budget to 128 MiB. A compressed file can be small on disk but still exceed the
decoding budget. If a preview is unavailable, check the format and try a smaller
copy of the image.
