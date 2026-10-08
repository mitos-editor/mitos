# Changelog

## [Unreleased]

## [0.1.0](https://github.com/mitos-editor/mitos/releases/tag/v0.1.0) - 2026-10-08

Mitos's first release builds on Helix's selection-first editing, multiple
selections, syntax highlighting, language servers, and debugging. These are
the main additions and changes since the fork, including work adapted from
the upstream Helix PRs linked below.

### Added

- [Built-in spell checking](https://mitos.computer/docs/spell-checking.html) for prose and comments, with correction suggestions,
  personal dictionaries, and temporary or permanent word ignores
  ([Helix #15910](https://github.com/helix-editor/helix/pull/15910)).
- [Image previews](https://mitos.computer/docs/image-previews.html) in editor panes and file pickers for PNG, JPEG, GIF, and WebP
  ([#9](https://github.com/mitos-editor/mitos/pull/9)). Previews automatically use
  [Kitty graphics](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
  [Sixel](https://vt100.net/docs/vt3xx-gp/chapter14.html), or
  [iTerm2 inline images](https://iterm2.com/documentation-images.html), depending
  on terminal support, with a half-block character fallback. GIF and WebP
  previews show a still image.
- A [quicklist](https://mitos.computer/docs/quicklist.html) to collect search results, diagnostics, or symbol locations and
  move through them after closing the picker.
- A [picker for changed Git files](https://mitos.computer/docs/keymap.html#space-mode) in the current workspace.
- [Breadcrumb navigation](https://mitos.computer/docs/editor.html#editorbreadcrumb-section) showing the file path and enclosing symbols
  ([Helix #15573](https://github.com/helix-editor/helix/pull/15573)).
- [Custom `:` commands](https://mitos.computer/docs/custom-commands.html) with aliases, command sequences, macros, and arguments.
- [Automatic reloads](https://mitos.computer/docs/configuration.html#editorauto-reload) when files change outside the editor
  ([Helix #14544](https://github.com/helix-editor/helix/pull/14544)).
- [`Alt-r`](https://mitos.computer/docs/keymap.html#changes) to replace selections with entered text and repeat the edit with `.`.
- [Select all matching text objects](https://mitos.computer/docs/keymap.html#match-mode) within a selection
  ([Helix #16088](https://github.com/helix-editor/helix/pull/16088)) and select
  the matched text when opening global search results
  ([Helix #16061](https://github.com/helix-editor/helix/pull/16061)).
- [Toggle breakpoints](https://mitos.computer/docs/debugging.html#debugging-commands) across multiple selections
  ([Helix #16164](https://github.com/helix-editor/helix/pull/16164)).

### Changed

- Rebuilt the terminal interface with Ratatui, with a welcome screen, optional
  file icons, clearer panels, updated [pickers](https://mitos.computer/docs/pickers.html), and a simpler
  [status line](https://mitos.computer/docs/editor.html#editorstatusline-section).
- Command help supports Markdown, and symbol lists use syntax-aware colors.
- Modus Operandi and Modus Vivendi are the default light and dark
  [themes](https://mitos.computer/docs/themes.html).
  The bundled theme collection is smaller.
- The executable is `ms`. [User configuration](https://mitos.computer/docs/configuration.html#configuration-files-and-precedence) lives in `~/.config/mitos`
  (`%AppData%\mitos` on Windows), and project configuration uses `.mitos/`.
- Custom keymaps that used `replace` for character replacement must use
  `replace_char`. The default `r` binding still replaces characters.
- Reorganized the codebase, updated dependencies, and refreshed the [documentation](https://mitos.computer/docs/)
  ([#75](https://github.com/mitos-editor/mitos/pull/75)).

### Fixed

- Reduced blocking work during startup
  ([#17](https://github.com/mitos-editor/mitos/pull/17)), redraws, and code actions
  ([#84](https://github.com/mitos-editor/mitos/pull/84)).
- Improved terminal cursor handling
  ([#101](https://github.com/mitos-editor/mitos/pull/101)), [file explorer](https://mitos.computer/docs/pickers.html#file-explorer)
  navigation, and Windows file watching
  ([#103](https://github.com/mitos-editor/mitos/pull/103)), state paths, and
  line endings.
- Made comment joining syntax-aware
  ([Helix #15992](https://github.com/helix-editor/helix/pull/15992)) and corrected
  formatting defaults on save.
