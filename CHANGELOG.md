# Changelog

## [Unreleased]

## [0.1.1](https://github.com/mitos-editor/mitos/compare/v0.1.0...v0.1.1) - 2026-10-20

### Added

- *(debugger)* show inline runtime values in source ([#511](https://github.com/mitos-editor/mitos/pull/511))
- *(debugger)* inspect evaluated values through owned variable pages
- *(debugger)* support breakpoints in adapter-owned sources
- *(debugger)* page variable children on demand
- *(debugger)* refresh invalidated inspection data in place
- *(debugger)* browse scopes and nested variables lazily
- *(debugger)* add session switching and selected-session status
- *(picker)* refresh symbols and unsaved search results
- *(picker)* refresh buffers and diagnostics from editor state
- *(picker)* refresh search matches and changed-file status
- *(picker)* refresh file lists without losing selection
- *(vcs)* add blame details and clarify change restoration ([#388](https://github.com/mitos-editor/mitos/pull/388))
- *(lsp)* add ordered server alternatives and expand language coverage ([#342](https://github.com/mitos-editor/mitos/pull/342))
- inline git blame ([#107](https://github.com/mitos-editor/mitos/pull/107))
- *(keymap)* add configurable menu and command labels ([#134](https://github.com/mitos-editor/mitos/pull/134))
- *(all)* restructure configuration ([#122](https://github.com/mitos-editor/mitos/pull/122))
- *(ui)* make statusline layout configurable ([#127](https://github.com/mitos-editor/mitos/pull/127))
- add component keymaps and buffer picker close action ([#126](https://github.com/mitos-editor/mitos/pull/126))
- *(lsp)* code lenses ([#121](https://github.com/mitos-editor/mitos/pull/121))
- *(lsp)* support inline completion ghost text ([#117](https://github.com/mitos-editor/mitos/pull/117))
- *(view)* support undoable document-only transactions ([#303](https://github.com/mitos-editor/mitos/pull/303))
- *(loader)* discover runtimes in the user data directory
- *(lsp)* cancel abandoned and timed-out requests ([#298](https://github.com/mitos-editor/mitos/pull/298))
- *(debugger)* make session selection explicit across adapter lifetimes
- *(terminal)* detect Kitty and Alacritty launchers ([#315](https://github.com/mitos-editor/mitos/pull/315))
- *(config)* configure zellij as terminal.launcher if running inside it ([#314](https://github.com/mitos-editor/mitos/pull/314))
- *(xtask)* automate weekly grammar updates ([#294](https://github.com/mitos-editor/mitos/pull/294))

### Fixed

- *(editor)* scope file preflight guards to affected documents
- *(grammar)* write installed grammars to the user data runtime
- *(debugger)* integrate frame paging and adapter sources into main
- *(debugger)* map source breakpoints through document edits
- *(debugger)* wait for managed TCP adapter readiness
- *(debugger)* model thread execution and honor continue responses ([#460](https://github.com/mitos-editor/mitos/pull/460))
- *(debugger)* normalize UTF-16 source locations and validate lines ([#461](https://github.com/mitos-editor/mitos/pull/461))
- *(debugger)* own reverse requests and support stdio child sessions
- *(debugger)* end sessions when adapter transports close
- *(picker)* preserve navigation during background refreshes
- *(watcher)* track Git index changes without polling
- *(picker)* publish refresh results during continuous changes ([#428](https://github.com/mitos-editor/mitos/pull/428))
- *(keymap)* use space "E" to open current folder (reverts space ".") ([#427](https://github.com/mitos-editor/mitos/pull/427))
- *(picker)* refresh cached previews after filesystem changes
- *(vcs)* include staged changes in changed-file pickers ([#401](https://github.com/mitos-editor/mitos/pull/401))
- *(indent)* in gdscript newline after a block's last line had no indent ([#374](https://github.com/mitos-editor/mitos/pull/374))
- *(term)* bind queued UI work to its originating session ([#354](https://github.com/mitos-editor/mitos/pull/354))
- *(lsp)* open documents after editor initialization exactly once ([#351](https://github.com/mitos-editor/mitos/pull/351))
- *(save)* drain accepted jobs and writes after failures ([#305](https://github.com/mitos-editor/mitos/pull/305))
- *(format)* reject callbacks after view rebinding ([#301](https://github.com/mitos-editor/mitos/pull/301))
- *(term)* initialize frontend destinations and hooks once ([#331](https://github.com/mitos-editor/mitos/pull/331))
- *(term)* race condition on config tests
- *(palette)* deliver command continuations after closing ([#304](https://github.com/mitos-editor/mitos/pull/304))
- *(config)* make configuration migration explicit ([#135](https://github.com/mitos-editor/mitos/pull/135))
- *(lsp)* clear diagnostics when restarting or stopping servers ([#118](https://github.com/mitos-editor/mitos/pull/118))
- *(buffer)* advance text and history revisions together during replay
- *(dap)* preserve and format structured request errors
- *(dap)* sequence all outgoing messages in queue order
- *(debugger)* apply session capability updates
- *(debugger)* synchronize breakpoints with saved and renamed sources
- *(editor)* enforce resource destination policies ([#507](https://github.com/mitos-editor/mitos/pull/507))
- *(editor)* reserve save paths through completion delivery
- *(editor)* reject malformed document edit groups ([#508](https://github.com/mitos-editor/mitos/pull/508))
- *(debugger)* suspend configuration work during shutdown
- *(debugger)* reject and cancel launch requests during shutdown ([#481](https://github.com/mitos-editor/mitos/pull/481))
- *(debugger)* isolate asynchronous results across session switches
- *(registers)* preserve clipboard contents and failed-write state ([#412](https://github.com/mitos-editor/mitos/pull/412))
- *(view)* preserve document lifecycle event identities ([#300](https://github.com/mitos-editor/mitos/pull/300))
- detect file encoding using only bytes actually read ([#114](https://github.com/mitos-editor/mitos/pull/114))

### Other

- *(editor)* clarify file mutation outcomes and effects
- *(debugger)* share adapter protocol fixtures and request gates
- *(debugger)* share launch template resolution across frontends ([#479](https://github.com/mitos-editor/mitos/pull/479))
- *(ui)* settle picker renders and wait for completion providers ([#472](https://github.com/mitos-editor/mitos/pull/472))
- *(debugger)* separate launch configuration from DAP types
- *(debugger)* keep adapter clients behind the session API
- generate the default keymap reference ([#463](https://github.com/mitos-editor/mitos/pull/463))
- *(completion)* own menu and preview lifecycle in the shared service ([#445](https://github.com/mitos-editor/mitos/pull/445))
- *(debugger)* own adapter lifetimes and execution state in editor sessions
- *(debugger)* connect and initialize adapters without blocking the editor
- Use binary placeholders when image protocols are unavailable ([#444](https://github.com/mitos-editor/mitos/pull/444))
- *(symbols)* own document picker queries in the editor
- *(symbols)* own workspace providers and cancel picker queries
- *(actions)* coordinate interactive providers in the editor
- *(rename)* own asynchronous preparation and document-bound edits
- *(completion)* centralize preview and acceptance edits ([#438](https://github.com/mitos-editor/mitos/pull/438))
- *(search)* share execution for picker queries and refreshes
- *(debugger)* synchronize source breakpoints asynchronously
- *(shell)* run selection filters as owned asynchronous tasks ([#437](https://github.com/mitos-editor/mitos/pull/437))
- *(completion)* resolve accepted edits asynchronously ([#435](https://github.com/mitos-editor/mitos/pull/435))
- *(files)* coordinate language-server preflight asynchronously ([#434](https://github.com/mitos-editor/mitos/pull/434))
- *(words)* extract incremental Unicode word indexing ([#414](https://github.com/mitos-editor/mitos/pull/414))
- *(debugger)* own asynchronous controls and session-bound inspection
- *(debugger)* validate and launch sessions through editor operations
- *(fuzzy)* extract matching policies and reusable consumer state ([#409](https://github.com/mitos-editor/mitos/pull/409))
- *(editor)* own document language and dictionary changes
- *(diagnostics)* extract shared values and edit-mapped collections
- *(diagnostics)* own storage mapping and protocol publication
- *(registers)* separate storage from editor context resolution ([#410](https://github.com/mitos-editor/mitos/pull/410))
- *(document)* encapsulate file state and persistence ([#405](https://github.com/mitos-editor/mitos/pull/405))
- *(language)* own query feature metadata ([#404](https://github.com/mitos-editor/mitos/pull/404))
- *(language-config)* own spelling settings and dictionary identifiers ([#403](https://github.com/mitos-editor/mitos/pull/403))
- *(theme)* own icon lookup and presentation values ([#400](https://github.com/mitos-editor/mitos/pull/400))
- *(completion)* own provider-bound item models in editor feature ([#396](https://github.com/mitos-editor/mitos/pull/396))
- *(editing)* share paste and register replacement operations ([#399](https://github.com/mitos-editor/mitos/pull/399))
- *(layout)* distinguish display positions from document coordinates
- *(config)* extract editor settings into editor-config
- *(config)* separate settings data from editor runtime behavior
- *(selection)* share document selection transformations ([#392](https://github.com/mitos-editor/mitos/pull/392))
- *(buffer)* own text history implementation and navigation types ([#375](https://github.com/mitos-editor/mitos/pull/375))
- *(layout)* extract renderer-independent text layout ([#384](https://github.com/mitos-editor/mitos/pull/384))
- *(core)* separate text and display coordinates ([#383](https://github.com/mitos-editor/mitos/pull/383))
- *(lsp)* keep server selection and roots in editor integration
- *(lsp)* move editor edit and snippet conversions into view
- *(language)* extract parser-independent configuration crate
- *(editor)* restrict structural document and view mutations ([#386](https://github.com/mitos-editor/mitos/pull/386))
- *(editing)* share insertion and deletion operations ([#385](https://github.com/mitos-editor/mitos/pull/385))
- *(editing)* share transformations and language-aware operations ([#380](https://github.com/mitos-editor/mitos/pull/380))
- *(document)* own state for each document and view together
- *(text-io)* extract stream encoding and decoding ([#363](https://github.com/mitos-editor/mitos/pull/363))
- *(search)* extract workspace search engine ([#362](https://github.com/mitos-editor/mitos/pull/362))
- *(clipboard)* extract provider settings and native backends ([#361](https://github.com/mitos-editor/mitos/pull/361))
- *(grammar)* separate native grammar build tooling ([#360](https://github.com/mitos-editor/mitos/pull/360))
- *(language)* extract syntax and structural editing from core
- *(core)* separate text primitives from structural editing
- *(theme)* extract theme interpretation from editor state
- *(workspace)* extract root discovery and trust policy
- *(debugger)* delegate terminal launches to the frontend ([#353](https://github.com/mitos-editor/mitos/pull/353))
- *(view)* own mode transitions and session effects in the editor ([#352](https://github.com/mitos-editor/mitos/pull/352))
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
- *(buffer)* share immutable snapshots across document work
- *(buffer)* extract text state and undo ownership into a crate
- *(diff)* extract text comparison and reusable line differencing ([#413](https://github.com/mitos-editor/mitos/pull/413))
- *(document)* own EditorConfig discovery outside text core ([#397](https://github.com/mitos-editor/mitos/pull/397))
- *(dap)* exercise a pinned Delve adapter in CI
- *(dap)* validate typed replies for standard commands
- *(debugger)* own capabilities and launch settings in sessions
- *(debugger)* bind source operations to live document identities
- *(debugger)* make session lifecycle transitions explicit ([#477](https://github.com/mitos-editor/mitos/pull/477))
- *(debugger)* depend on protocol types without the async client
- *(debugger)* extract editor-independent state models
- *(language)* separate semantic coverage from interactive parse deadlines
- *(language)* drop serialization dependencies owned by configuration
- *(language)* resolve parser state through its owning loader
- *(loader)* compare runtime override paths by directory identity
- *(deps)* bump the rust-dependencies group across 1 directory with 31 updates ([#129](https://github.com/mitos-editor/mitos/pull/129))
- *(lsp)* organize client lifecycle and protocol implementations ([#418](https://github.com/mitos-editor/mitos/pull/418))
- *(lsp)* keep configured feature selection in editor integration ([#393](https://github.com/mitos-editor/mitos/pull/393))
- *(lsp)* restrict registry key dependency to tests
- *(lsp)* clarify ownership of language server identities
- *(lsp)* make registry lifecycle state explicit ([#345](https://github.com/mitos-editor/mitos/pull/345))
- *(spelling)* scan explicit text regions without parser dependencies ([#398](https://github.com/mitos-editor/mitos/pull/398))
- *(stdx)* isolate current-directory mutations ([#296](https://github.com/mitos-editor/mitos/pull/296))
- Add border style to theme ([#517](https://github.com/mitos-editor/mitos/pull/517))
- *(debugger)* separate request ownership from commands
- *(debugger)* separate session and inspection request lifetimes
- *(debugger)* process execution events asynchronously
- *(debugger)* separate source breakpoints from adapter bindings
- *(buffer)* return applied changes before document effects
- *(document)* encapsulate buffer text state and history
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
