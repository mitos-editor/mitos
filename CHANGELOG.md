# Changelog

## [Unreleased]

## [0.1.1](https://github.com/mitos-editor/mitos/compare/v0.1.0...v0.1.1) - 2026-10-17

### Added

- *(lsp)* add ordered server alternatives and expand language coverage ([#342](https://github.com/mitos-editor/mitos/pull/342))
- inline git blame ([#107](https://github.com/mitos-editor/mitos/pull/107))
- *(keymap)* add configurable menu and command labels ([#134](https://github.com/mitos-editor/mitos/pull/134))
- *(all)* restructure configuration ([#122](https://github.com/mitos-editor/mitos/pull/122))
- *(ui)* make statusline layout configurable ([#127](https://github.com/mitos-editor/mitos/pull/127))
- add component keymaps and buffer picker close action ([#126](https://github.com/mitos-editor/mitos/pull/126))
- *(lsp)* code lenses ([#121](https://github.com/mitos-editor/mitos/pull/121))
- *(lsp)* support inline completion ghost text ([#117](https://github.com/mitos-editor/mitos/pull/117))
- *(view)* support undoable document-only transactions ([#303](https://github.com/mitos-editor/mitos/pull/303))
- *(lsp)* cancel abandoned and timed-out requests ([#298](https://github.com/mitos-editor/mitos/pull/298))
- *(terminal)* detect Kitty and Alacritty launchers ([#315](https://github.com/mitos-editor/mitos/pull/315))
- *(config)* configure zellij as terminal.launcher if running inside it ([#314](https://github.com/mitos-editor/mitos/pull/314))
- *(xtask)* automate weekly grammar updates ([#294](https://github.com/mitos-editor/mitos/pull/294))

### Fixed

- *(save)* drain accepted jobs and writes after failures ([#305](https://github.com/mitos-editor/mitos/pull/305))
- *(format)* reject callbacks after view rebinding ([#301](https://github.com/mitos-editor/mitos/pull/301))
- *(term)* initialize frontend destinations and hooks once ([#331](https://github.com/mitos-editor/mitos/pull/331))
- *(term)* race condition on config tests
- *(palette)* deliver command continuations after closing ([#304](https://github.com/mitos-editor/mitos/pull/304))
- *(config)* make configuration migration explicit ([#135](https://github.com/mitos-editor/mitos/pull/135))
- *(lsp)* clear diagnostics when restarting or stopping servers ([#118](https://github.com/mitos-editor/mitos/pull/118))
- *(view)* preserve document lifecycle event identities ([#300](https://github.com/mitos-editor/mitos/pull/300))
- detect file encoding using only bytes actually read ([#114](https://github.com/mitos-editor/mitos/pull/114))

### Other

- *(view)* coordinate scratch-file loading through the editor ([#349](https://github.com/mitos-editor/mitos/pull/349))
- *(picker)* own preview loading and rendering in one module ([#346](https://github.com/mitos-editor/mitos/pull/346))
- *(navigation)* share file-link resolution and validate its origin ([#350](https://github.com/mitos-editor/mitos/pull/350))
- *(save)* own ordered write queues and completion accounting ([#324](https://github.com/mitos-editor/mitos/pull/324))
- *(view)* own workspace edits and preserve rejection causes ([#337](https://github.com/mitos-editor/mitos/pull/337))
- *(term)* own goto previews in command prompt sessions ([#339](https://github.com/mitos-editor/mitos/pull/339))
- *(spelling)* own dictionary resources in feature handler ([#338](https://github.com/mitos-editor/mitos/pull/338))
- *(editing)* separate document history from view selections ([#336](https://github.com/mitos-editor/mitos/pull/336))
- *(save)* own the complete save pipeline in view ([#319](https://github.com/mitos-editor/mitos/pull/319))
- *(view)* finish feature ownership and remove handler aliases ([#333](https://github.com/mitos-editor/mitos/pull/333))
- own editor status and redraw signals ([#326](https://github.com/mitos-editor/mitos/pull/326))
- *(view)* share view-bound request snapshots ([#330](https://github.com/mitos-editor/mitos/pull/330))
- *(view)* coordinate language navigation requests ([#325](https://github.com/mitos-editor/mitos/pull/325))
- *(view)* establish document invariants before observers ([#321](https://github.com/mitos-editor/mitos/pull/321))
- own frontend callbacks and event subscriptions ([#318](https://github.com/mitos-editor/mitos/pull/318))
- *(view)* group feature state and coordination ([#320](https://github.com/mitos-editor/mitos/pull/320))
- *(view)* own hover and inlay hint requests ([#317](https://github.com/mitos-editor/mitos/pull/317))
- *(view)* own debugger state and breakpoint mutations ([#316](https://github.com/mitos-editor/mitos/pull/316))
- startup optimization ([#119](https://github.com/mitos-editor/mitos/pull/119))
- *(deps)* bump the rust-dependencies group across 1 directory with 31 updates ([#129](https://github.com/mitos-editor/mitos/pull/129))
- *(lsp)* make registry lifecycle state explicit ([#345](https://github.com/mitos-editor/mitos/pull/345))
- *(stdx)* isolate current-directory mutations ([#296](https://github.com/mitos-editor/mitos/pull/296))
- update Cargo.toml dependencies
- *(editor)* group document lifecycle and filesystem operations ([#340](https://github.com/mitos-editor/mitos/pull/340))
- *(view)* co-locate cached document features ([#329](https://github.com/mitos-editor/mitos/pull/329))
- *(view)* separate document text encoding from model state ([#328](https://github.com/mitos-editor/mitos/pull/328))
- *(diagnostics)* replace language-server findings by provider ([#299](https://github.com/mitos-editor/mitos/pull/299))

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
