# Changelog

## [Unreleased]

## [0.1.3](https://github.com/mitos-editor/mitos/compare/v0.1.2...v0.1.3) - 2026-10-25

### Added

- *(commands)* add interactive shell expansions ([#780](https://github.com/mitos-editor/mitos/pull/780))
- *(keymap)* add shifted-arrow split swap bindings ([#769](https://github.com/mitos-editor/mitos/pull/769))
- *(search)* show search position and match count in the statusline ([#772](https://github.com/mitos-editor/mitos/pull/772))
- *(vcs)* honor search wrapping in change navigation ([#725](https://github.com/mitos-editor/mitos/pull/725))
- *(editing)* extend selection sorting options ([#773](https://github.com/mitos-editor/mitos/pull/773))
- *(palette)* show typable command aliases ([#731](https://github.com/mitos-editor/mitos/pull/731))
- *(diagnostics)* distinguish plain severity markers by shape ([#767](https://github.com/mitos-editor/mitos/pull/767))
- *(commands)* show Unicode categories in character info ([#682](https://github.com/mitos-editor/mitos/pull/682))
- *(keymap)* bind reverse split rotation ([#698](https://github.com/mitos-editor/mitos/pull/698))
- *(ruby)* add method-call argument textobjects ([#667](https://github.com/mitos-editor/mitos/pull/667))
- *(install)* copy runtime assets when installing from source ([#658](https://github.com/mitos-editor/mitos/pull/658))
- *(picker)* refresh buffers and diagnostics from editor state
- *(debugger)* support breakpoints in adapter-owned sources
- *(debugger)* refresh invalidated inspection data in place
- *(debugger)* show inline runtime values in source ([#511](https://github.com/mitos-editor/mitos/pull/511))
- *(theme)* add border style support ([#517](https://github.com/mitos-editor/mitos/pull/517))
- *(snippets)* support split collections and punctuation triggers
- *(snippets)* complete native insertion, workspace scopes, and variables
- *(snippets)* load user definitions into native completion
- *(editor)* support native block cursors ([#623](https://github.com/mitos-editor/mitos/pull/623))
- *(rulers)* port configurable foreground rulers from Helix ([#630](https://github.com/mitos-editor/mitos/pull/630))
- *(picker)* add centered layout preference ([#624](https://github.com/mitos-editor/mitos/pull/624))
- *(editor)* render trailing whitespace independently ([#540](https://github.com/mitos-editor/mitos/pull/540))
- *(debugger)* add session switching and selected-session status

### Fixed

- *(search)* preserve invalid-pattern details and clear stale errors ([#734](https://github.com/mitos-editor/mitos/pull/734))
- *(health)* report runtime problems for language checks ([#740](https://github.com/mitos-editor/mitos/pull/740))
- *(lsp)* reveal the cursor after jumping to a location ([#738](https://github.com/mitos-editor/mitos/pull/738))
- *(macros)* clear pending counts after outermost replay ([#676](https://github.com/mitos-editor/mitos/pull/676))
- *(uri)* recognize file schemes without case sensitivity ([#695](https://github.com/mitos-editor/mitos/pull/695))
- *(render)* keep jump labels above syntax decorations ([#704](https://github.com/mitos-editor/mitos/pull/704))
- *(kotlin)* keep else bodies at the expected indent ([#711](https://github.com/mitos-editor/mitos/pull/711))
- *(picker)* preserve filenames in narrow path columns ([#661](https://github.com/mitos-editor/mitos/pull/661))
- *(popup)* keep clamped scrolling stable across redraws ([#663](https://github.com/mitos-editor/mitos/pull/663))
- *(rulers)* preserve glyphs beneath hidden whitespace ([#656](https://github.com/mitos-editor/mitos/pull/656))
- allow disabling popup borders ([#655](https://github.com/mitos-editor/mitos/pull/655))
- *(buffer)* advance text and history revisions together during replay
- *(comments)* count Unicode tokens in character offsets ([#675](https://github.com/mitos-editor/mitos/pull/675))
- *(wrapping)* keep wide graphemes inside the viewport ([#696](https://github.com/mitos-editor/mitos/pull/696))
- *(increment)* prevent hangs and preserve numeric formatting ([#677](https://github.com/mitos-editor/mitos/pull/677))
- *(debugger)* integrate frame paging and adapter sources into main
- *(debugger)* synchronize breakpoints with saved and renamed sources
- *(debugger)* map source breakpoints through document edits
- *(grammar)* check out the fetched grammar revision ([#684](https://github.com/mitos-editor/mitos/pull/684))
- *(grammar)* write installed grammars to the user data runtime
- *(go)* indent incomplete case bodies on a new line ([#753](https://github.com/mitos-editor/mitos/pull/753))
- *(indent)* in gdscript newline after a block's last line had no indent ([#374](https://github.com/mitos-editor/mitos/pull/374))
- save and format ([#76](https://github.com/mitos-editor/mitos/pull/76))
- *(theme)* use a widely supported unverified breakpoint glyph ([#670](https://github.com/mitos-editor/mitos/pull/670))
- *(picker)* remove extra left padding when using icons ([#640](https://github.com/mitos-editor/mitos/pull/640))
- *(tui)* preserve terminal-managed backgrounds ([#733](https://github.com/mitos-editor/mitos/pull/733))
- *(tui)* restore terminal modes only while claimed ([#668](https://github.com/mitos-editor/mitos/pull/668))
- *(files)* create parent directories before moving a file ([#736](https://github.com/mitos-editor/mitos/pull/736))
- *(dap)* request termination through disconnect when supported ([#712](https://github.com/mitos-editor/mitos/pull/712))
- *(rulers)* preserve background rulers with explicit styles ([#633](https://github.com/mitos-editor/mitos/pull/633))

### Other

- *(compositor)* use type identity for component lookup ([#699](https://github.com/mitos-editor/mitos/pull/699))
- *(buffer)* own text savepoint tracking
- *(buffer)* own text history implementation and navigation types ([#375](https://github.com/mitos-editor/mitos/pull/375))
- *(buffer)* share immutable snapshots across document work
- *(buffer)* extract text state and undo ownership into a crate
- *(clipboard)* extract provider settings and native backends ([#361](https://github.com/mitos-editor/mitos/pull/361))
- *(completion)* isolate previews from live document edits
- *(text)* migrate the workspace to Ropey 2 ([#583](https://github.com/mitos-editor/mitos/pull/583))
- *(diagnostics)* extract shared values and edit-mapped collections
- *(debugger)* bind source operations to live document identities
- *(debugger)* make session lifecycle transitions explicit ([#477](https://github.com/mitos-editor/mitos/pull/477))
- *(debugger)* depend on protocol types without the async client
- *(debugger)* extract editor-independent state models
- *(grammar)* separate native grammar build tooling ([#360](https://github.com/mitos-editor/mitos/pull/360))
- *(textobjects)* move range collection into core and language
- *(symbols)* separate syntax queries from picker presentation
- *(language)* separate semantic coverage from interactive parse deadlines
- *(language)* own query feature metadata ([#404](https://github.com/mitos-editor/mitos/pull/404))
- *(core)* separate text and display coordinates ([#383](https://github.com/mitos-editor/mitos/pull/383))
- *(language)* drop serialization dependencies owned by configuration
- *(language)* extract parser-independent configuration crate
- *(language)* resolve parser state through its owning loader
- *(language)* extract syntax and structural editing from core
- *(debugger)* share launch template resolution across frontends ([#479](https://github.com/mitos-editor/mitos/pull/479))
- *(debugger)* separate launch configuration from DAP types
- *(language-config)* own spelling settings and dictionary identifiers ([#403](https://github.com/mitos-editor/mitos/pull/403))
- *(search)* share workspace traversal policy
- *(search)* extract workspace search engine ([#362](https://github.com/mitos-editor/mitos/pull/362))
- *(text-io)* extract stream encoding and decoding ([#363](https://github.com/mitos-editor/mitos/pull/363))
- *(config)* move border style to editor settings ([#521](https://github.com/mitos-editor/mitos/pull/521))
- *(theme)* own icon lookup and presentation values ([#400](https://github.com/mitos-editor/mitos/pull/400))
- *(core)* separate text primitives from structural editing
- *(theme)* extract theme interpretation from editor state
- *(workspace)* extract root discovery and trust policy
- *(completion)* index the latest document snapshots
- *(word-index)* check cancellation during input scanning ([#575](https://github.com/mitos-editor/mitos/pull/575))
- *(words)* extract incremental Unicode word indexing ([#414](https://github.com/mitos-editor/mitos/pull/414))
- *(watcher)* extract native file watching into file-watch ([#525](https://github.com/mitos-editor/mitos/pull/525))
- *(layout)* extract document-independent pane layout
- *(symbols)* extract syntax-symbols engine crate ([#613](https://github.com/mitos-editor/mitos/pull/613))
- *(layout)* distinguish display positions from document coordinates
- *(layout)* extract renderer-independent text layout ([#384](https://github.com/mitos-editor/mitos/pull/384))
- *(config)* default to rounded borders and trailing whitespace ([#577](https://github.com/mitos-editor/mitos/pull/577))
- *(rendering)* extract the text-render crate
- *(config)* extract editor settings into editor-config
- *(fuzzy)* extract matching policies and reusable consumer state ([#409](https://github.com/mitos-editor/mitos/pull/409))
- *(diff)* extract text comparison and reusable line differencing ([#413](https://github.com/mitos-editor/mitos/pull/413))

## [0.1.2](https://github.com/mitos-editor/mitos/compare/v0.1.1...v0.1.2) - 2026-10-23

### Highlights

- Define your own snippets without a language server. Insert them through completion, a picker, or keybindings, split collections across files, and share project snippets through trusted workspaces.
- User-defined and language-server snippets can insert selected text, filenames, workspace paths, clipboard contents, dates, and random values, with support for variable transforms.
- More UI customization: native terminal block cursors, centered pickers at any terminal size, and glyph rulers with configurable characters.
- Picker matching ignores decorative icons, icon spacing is tighter, and previews show unsaved changes when opening relative paths.
- Save-and-quit stays responsive while formatting and writing, preserves unsaved edits when writes fail, and waits for saves to finish before closing. External formatters have a configurable timeout, and LSP formatting uses the document's indentation width.
- Completion previews, repeated insertion, and nested snippet placeholders preserve edits, cursor positions, and navigation more reliably.
- Path-based settings refresh more consistently, and workspace symbol searches use the same case-matching rules for open buffers and files.
- Further internal restructuring shares editing, movement, search, and pane operations across frontends, separates rendering from input, and updates the text engine to Ropey 2.

### Upgrade notes

- Ruler settings now use `[editor.rulers]`, with `columns`, `style`, and `character` options. Existing `editor.rulers = [...]` arrays still work; `:config-migrate` updates them to the new format.

### Added

- *(quicklist)* add locations and remove entries from picker ([#638](https://github.com/mitos-editor/mitos/pull/638))
- *(snippets)* support split collections and punctuation triggers
- *(snippets)* complete native insertion, workspace scopes, and variables
- *(snippets)* load user definitions into native completion
- *(editor)* support native block cursors ([#623](https://github.com/mitos-editor/mitos/pull/623))
- *(rulers)* port configurable foreground rulers from Helix ([#630](https://github.com/mitos-editor/mitos/pull/630))
- *(picker)* add centered layout preference ([#624](https://github.com/mitos-editor/mitos/pull/624))
- *(picker)* refresh buffers and diagnostics from editor state
- *(debugger)* support breakpoints in adapter-owned sources
- *(debugger)* refresh invalidated inspection data in place
- *(debugger)* show inline runtime values in source ([#511](https://github.com/mitos-editor/mitos/pull/511))
- *(snippets)* resolve document variables from insertion snapshots ([#642](https://github.com/mitos-editor/mitos/pull/642))
- *(theme)* add border style support ([#517](https://github.com/mitos-editor/mitos/pull/517))
- *(editor)* render trailing whitespace independently ([#540](https://github.com/mitos-editor/mitos/pull/540))
- *(debugger)* add session switching and selected-session status

### Fixed

- *(picker)* preview unsaved buffers for relative paths ([#641](https://github.com/mitos-editor/mitos/pull/641))
- *(picker)* remove extra left padding when using icons ([#640](https://github.com/mitos-editor/mitos/pull/640))
- *(picker)* exclude decorative icons from fuzzy matching ([#637](https://github.com/mitos-editor/mitos/pull/637))
- *(rulers)* preserve background rulers with explicit styles ([#633](https://github.com/mitos-editor/mitos/pull/633))
- save and format ([#76](https://github.com/mitos-editor/mitos/pull/76))
- *(completion)* replay snippet edits and cursor state ([#601](https://github.com/mitos-editor/mitos/pull/601))
- *(buffer)* advance text and history revisions together during replay
- *(debugger)* integrate frame paging and adapter sources into main
- *(debugger)* synchronize breakpoints with saved and renamed sources
- *(debugger)* map source breakpoints through document edits
- *(grammar)* write installed grammars to the user data runtime
- *(indent)* in gdscript newline after a block's last line had no indent ([#374](https://github.com/mitos-editor/mitos/pull/374))
- *(snippets)* skip nested defaults removed by placeholder edits
- *(snippets)* preserve nested navigation and validate placeholder graphs
- *(settings)* refresh path overrides through one document owner ([#612](https://github.com/mitos-editor/mitos/pull/612))
- *(symbols)* apply one case policy to buffers and files

### Other

- *(snippets)* clarify ownership and consolidate editing state
- *(completion)* isolate previews from live document edits
- *(lsp)* keep registry mutation inside the editor
- *(reload)* own file notifications in editor operations
- *(panes)* complete splits and scroll explicit views ([#614](https://github.com/mitos-editor/mitos/pull/614))
- *(editor)* own document setting operations ([#603](https://github.com/mitos-editor/mitos/pull/603))
- *(search)* share buffer matching and editor coordination ([#587](https://github.com/mitos-editor/mitos/pull/587))
- *(editing)* separate view synchronization from text operations
- *(editor)* share snippet session operations ([#588](https://github.com/mitos-editor/mitos/pull/588))
- *(movement)* share ordinary text motions
- *(movement)* share character finds and primary selection rotation
- *(ui)* separate pane rendering from editor input
- *(input)* isolate checked insert replay
- *(buffer)* own text savepoint tracking
- *(editor)* share successful input effects
- *(editing)* use one fallible mutation contract
- *(text)* migrate the workspace to Ropey 2 ([#583](https://github.com/mitos-editor/mitos/pull/583))
- *(buffer)* own text history implementation and navigation types ([#375](https://github.com/mitos-editor/mitos/pull/375))
- *(buffer)* share immutable snapshots across document work
- *(buffer)* extract text state and undo ownership into a crate
- *(clipboard)* extract provider settings and native backends ([#361](https://github.com/mitos-editor/mitos/pull/361))
- *(diagnostics)* extract shared values and edit-mapped collections
- *(debugger)* bind source operations to live document identities
- *(debugger)* make session lifecycle transitions explicit ([#477](https://github.com/mitos-editor/mitos/pull/477))
- *(debugger)* depend on protocol types without the async client
- *(debugger)* extract editor-independent state models
- *(grammar)* separate native grammar build tooling ([#360](https://github.com/mitos-editor/mitos/pull/360))
- *(textobjects)* move range collection into core and language
- *(symbols)* separate syntax queries from picker presentation
- *(language)* separate semantic coverage from interactive parse deadlines
- *(language)* own query feature metadata ([#404](https://github.com/mitos-editor/mitos/pull/404))
- *(core)* separate text and display coordinates ([#383](https://github.com/mitos-editor/mitos/pull/383))
- *(language)* drop serialization dependencies owned by configuration
- *(language)* extract parser-independent configuration crate
- *(language)* resolve parser state through its owning loader
- *(language)* extract syntax and structural editing from core
- *(debugger)* share launch template resolution across frontends ([#479](https://github.com/mitos-editor/mitos/pull/479))
- *(debugger)* separate launch configuration from DAP types
- *(language-config)* own spelling settings and dictionary identifiers ([#403](https://github.com/mitos-editor/mitos/pull/403))
- *(snippets)* use native path components in collection assertions
- *(search)* share workspace traversal policy
- *(search)* extract workspace search engine ([#362](https://github.com/mitos-editor/mitos/pull/362))
- *(text-io)* extract stream encoding and decoding ([#363](https://github.com/mitos-editor/mitos/pull/363))
- *(config)* move border style to editor settings ([#521](https://github.com/mitos-editor/mitos/pull/521))
- *(theme)* own icon lookup and presentation values ([#400](https://github.com/mitos-editor/mitos/pull/400))
- *(core)* separate text primitives from structural editing
- *(theme)* extract theme interpretation from editor state
- *(snippets)* derive workspace source paths from the document
- *(completion)* index the latest document snapshots
- reuse default documents and debugger message framing ([#622](https://github.com/mitos-editor/mitos/pull/622))
- *(symbols)* extract syntax-symbols engine crate ([#613](https://github.com/mitos-editor/mitos/pull/613))
- *(symbols)* separate blocking workspace scans
- *(workspace)* extract root discovery and trust policy
- *(word-index)* check cancellation during input scanning ([#575](https://github.com/mitos-editor/mitos/pull/575))
- *(words)* extract incremental Unicode word indexing ([#414](https://github.com/mitos-editor/mitos/pull/414))
- *(watcher)* extract native file watching into file-watch ([#525](https://github.com/mitos-editor/mitos/pull/525))
- *(layout)* extract document-independent pane layout
- *(layout)* distinguish display positions from document coordinates
- *(layout)* extract renderer-independent text layout ([#384](https://github.com/mitos-editor/mitos/pull/384))
- *(config)* default to rounded borders and trailing whitespace ([#577](https://github.com/mitos-editor/mitos/pull/577))
- *(rendering)* extract the text-render crate
- *(config)* extract editor settings into editor-config
- *(fuzzy)* extract matching policies and reusable consumer state ([#409](https://github.com/mitos-editor/mitos/pull/409))
- *(diff)* extract text comparison and reusable line differencing ([#413](https://github.com/mitos-editor/mitos/pull/413))

## [0.1.1](https://github.com/mitos-editor/mitos/compare/v0.1.0...v0.1.1) - 2026-10-20

### Highlights

- Pickers update as files, Git changes, diagnostics, and search results
  change, while preserving your selection and navigation.
- Inline Git blame shows who last changed a line. A new blame popup
  provides full commit details, and changed-file pickers include staged files.
- Language servers can provide inline completion suggestions and code
  lenses, such as run and test actions beside your code.
- Debugging gains session switching, inline runtime values, expandable
  variables, and more reliable breakpoint tracking.
- More UI customization: configurable statusline layouts, component
  keybindings, menu labels, and border styles. Pickers have clearer titles,
  and the active buffer stays visible in the buffer bar.
- Updated language grammars and syntax queries, broader language-server
  configuration, and support for choosing the first available server.
- Shell commands and more language-server and debugger operations run
  without blocking editing. Startup performance also improves.
- Added shell and PowerShell installers, improved terminal detection,
  and reorganized documentation.
- Extensive internal restructuring separates editor features from the
  terminal interface, alongside fixes to saving, formatting, completion,
  clipboard handling, and asynchronous operations.

### Upgrade notes

- Configuration is now grouped by feature. Older setting names still work;
  `:config-migrate` updates them after confirmation and creates a backup.
- Rounded borders and visible trailing whitespace are now the defaults.
- `Space E` opens the current folder.
- Installed grammars now use the user data runtime directory. Existing
  configuration runtime overrides remain supported and take priority.

### Added

- *(bufferline)* keep the active buffer visible ([#543](https://github.com/mitos-editor/mitos/pull/543))
- *(editor)* render trailing whitespace independently ([#540](https://github.com/mitos-editor/mitos/pull/540))
- *(picker)* label built-in pickers with titles ([#541](https://github.com/mitos-editor/mitos/pull/541))
- *(commands)* evaluate shell expansions without blocking the editor
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
- *(theme)* add border style support ([#517](https://github.com/mitos-editor/mitos/pull/517))
- *(debugger)* make session selection explicit across adapter lifetimes
- *(terminal)* detect Kitty and Alacritty launchers ([#315](https://github.com/mitos-editor/mitos/pull/315))
- *(config)* configure zellij as terminal.launcher if running inside it ([#314](https://github.com/mitos-editor/mitos/pull/314))
- *(xtask)* automate weekly grammar updates ([#294](https://github.com/mitos-editor/mitos/pull/294))

### Fixed

- *(editing)* propagate interactive edit failures before follow-up effects
- *(completion)* preview additional text edits ([#561](https://github.com/mitos-editor/mitos/pull/561))
- *(ui)* keep key help above the statusline ([#551](https://github.com/mitos-editor/mitos/pull/551))
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
- *(formatting)* capture expansion context from the target document
- *(formatting)* own external-tool processes through cancellation
- *(editor)* reserve save paths through completion delivery
- *(editor)* reject malformed document edit groups ([#508](https://github.com/mitos-editor/mitos/pull/508))
- *(debugger)* suspend configuration work during shutdown
- *(debugger)* reject and cancel launch requests during shutdown ([#481](https://github.com/mitos-editor/mitos/pull/481))
- *(debugger)* isolate asynchronous results across session switches
- *(registers)* preserve clipboard contents and failed-write state ([#412](https://github.com/mitos-editor/mitos/pull/412))
- *(view)* preserve document lifecycle event identities ([#300](https://github.com/mitos-editor/mitos/pull/300))
- detect file encoding using only bytes actually read ([#114](https://github.com/mitos-editor/mitos/pull/114))

### Other

- *(editor)* share mode entry and scope append restoration to views
- *(registers)* share editor operations and preflight edits
- *(editing)* extract selection transformation transactions
- *(editing)* share surround edits and validate replacement previews
- *(editing)* share structural selection and navigation
- *(textobjects)* move range collection into core and language
- *(editing)* use document-owned syntax resources
- *(movement)* share visual motion and viewport operations
- *(layout)* prepare view layout once per operation
- *(annotations)* pass explicit diagnostic and preview inputs
- *(syntax)* give documents explicit resource snapshots
- *(rendering)* extract the text-render crate
- *(rendering)* pass explicit text rendering inputs
- *(application)* separate startup configuration and protocol adapters
- *(navigation)* share server-requested document opening
- *(lsp)* coordinate work progress in the shared editor
- *(gutters)* keep terminal presentation with the frontend
- *(rendering)* share document highlighting outside EditorView
- *(workspace)* share change tracking across caches and pickers ([#536](https://github.com/mitos-editor/mitos/pull/536))
- *(layout)* extract document-independent pane layout
- *(picker)* give result publication a dedicated owner ([#529](https://github.com/mitos-editor/mitos/pull/529))
- *(search)* move workspace requests into shared editor features ([#530](https://github.com/mitos-editor/mitos/pull/530))
- *(config)* separate schema composition and trusted loading ([#523](https://github.com/mitos-editor/mitos/pull/523))
- *(symbols)* separate syntax queries from picker presentation
- *(search)* share workspace traversal policy
- *(config)* move border style to editor settings ([#521](https://github.com/mitos-editor/mitos/pull/521))
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
- *(ui)* depend on ratatui-core for shared rendering types ([#526](https://github.com/mitos-editor/mitos/pull/526))
- *(view)* reuse resolved paths between workspace changes ([#572](https://github.com/mitos-editor/mitos/pull/572))
- *(watcher)* extract native file watching into file-watch ([#525](https://github.com/mitos-editor/mitos/pull/525))
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
- *(word-index)* check cancellation during input scanning ([#575](https://github.com/mitos-editor/mitos/pull/575))
- *(config)* default to rounded borders and trailing whitespace ([#577](https://github.com/mitos-editor/mitos/pull/577))

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
