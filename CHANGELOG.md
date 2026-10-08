# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/mitos-editor/mitos/releases/tag/v0.1.0) - 2026-10-08

### Added

- *(docs)* update docs and READMEs ([#75](https://github.com/mitos-editor/mitos/pull/75))
- docs, screenshots, brand, and more
- images support ([#9](https://github.com/mitos-editor/mitos/pull/9))
- *(editor)* [**breaking**] add prompted selection replacement
- *(editor)* expand spell-checking syntax coverage
- *(editor)* persist spelling ignores across sessions
- *(editor)* add session spelling ignores
- *(editor)* add spelling navigation and text objects
- *(editor)* add native spell checking
- *(editor)* add file watching and automatic reload
- *(ui)* simplify statusline
- *(vcs)* add repository changed file picker
- show file icons in quicklist
- add quicklist
- *(lsp)* render symbol hierarchy with semantic styles
- *(commands)* add subselect commands for textobjects
- *(commands)* add custom typable commands
- *(ui)* highlight command palette entries
- *(ui)* render command docs as markdown
- *(editor)* add breadcrumb navigation
- *(dap)* toggle breakpoints for all selections
- *(term)* select global search matches
- enable automatic document highlights by default
- improve main application layout
- add welcome screen
- add basic icon support

### Fixed

- *(term)* make cursor ownership explicit ([#101](https://github.com/mitos-editor/mitos/pull/101))
- *(view)* resolve code actions without blocking the editor ([#84](https://github.com/mitos-editor/mitos/pull/84))
- respect editor auto-format defaults
- *(term)* avoid blocking cursor queries during full redraw
- repo name
- *(term)* remove directory collapsing from file explorer
- *(windows)* resolve state directory and line ending failures
- *(editor)* expose spelling corrections in code actions
- *(ui)* unify panel border styling
- *(vcs)* scope changed files to workspace
- *(vcs)* show changed files from worktree root
- *(ui)* preserve statusline filename width
- *(ui)* improve panel border contrast
- make picker results fill available width
- avoid quicklist panic from scratch buffer
- *(ui)* navigate file explorer in place
- *(rust)* adopt 2024 macro expressions
- *(ui)* hide breadcrumbs in scratch buffers
- *(commands)* make joining comments syntax aware
- restore CI checks

### Other

- *(view)* make handler lifecycle ownership explicit ([#85](https://github.com/mitos-editor/mitos/pull/85))
- *(tests)* share fake language server configuration ([#83](https://github.com/mitos-editor/mitos/pull/83))
- *(tests)* consolidate editor fixtures and feature coverage ([#82](https://github.com/mitos-editor/mitos/pull/82))
- *(term)* remove transitional command forwarding layers ([#88](https://github.com/mitos-editor/mitos/pull/88))
- *(view)* remove transitional configuration and spelling exports ([#81](https://github.com/mitos-editor/mitos/pull/81))
- *(view)* group document state by feature ([#79](https://github.com/mitos-editor/mitos/pull/79))
- *(view)* separate theme resources and own workspace trust ([#70](https://github.com/mitos-editor/mitos/pull/70))
- *(term)* own icon rendering into terminal spans ([#69](https://github.com/mitos-editor/mitos/pull/69))
- *(clipboard)* separate settings from runtime execution ([#68](https://github.com/mitos-editor/mitos/pull/68))
- *(syntax)* separate resource loading from query compilation ([#67](https://github.com/mitos-editor/mitos/pull/67))
- *(view)* own filesystem watching and event delivery ([#65](https://github.com/mitos-editor/mitos/pull/65))
- *(view)* own shared handler setup and event registration ([#64](https://github.com/mitos-editor/mitos/pull/64))
- *(view)* own completion coordination ([#60](https://github.com/mitos-editor/mitos/pull/60))
- *(view)* own signature-help coordination ([#58](https://github.com/mitos-editor/mitos/pull/58))
- *(view)* own autosave coordination ([#57](https://github.com/mitos-editor/mitos/pull/57))
- *(view)* own automatic reload coordination ([#56](https://github.com/mitos-editor/mitos/pull/56))
- *(view)* own editor configuration application ([#55](https://github.com/mitos-editor/mitos/pull/55))
- *(lsp)* clarify workspace request ownership ([#54](https://github.com/mitos-editor/mitos/pull/54))
- *(view)* own LSP lifecycle and push diagnostics ([#53](https://github.com/mitos-editor/mitos/pull/53))
- *(view)* own code-action hint coordination ([#52](https://github.com/mitos-editor/mitos/pull/52))
- *(view)* own pull-diagnostic coordination ([#51](https://github.com/mitos-editor/mitos/pull/51))
- *(view)* consolidate document LSP feature coordination ([#50](https://github.com/mitos-editor/mitos/pull/50))
- *(view)* consolidate spelling coordination ([#47](https://github.com/mitos-editor/mitos/pull/47))
- *(view)* own background syntax coordination ([#45](https://github.com/mitos-editor/mitos/pull/45))
- *(view)* extract shared selection replacement ([#43](https://github.com/mitos-editor/mitos/pull/43))
- *(view)* share save preparation across commands and autosave ([#41](https://github.com/mitos-editor/mitos/pull/41))
- *(term)* organize commands by feature
- *(term)* extract navigation commands
- *(term)* extract formatting command coordination
- *(term)* extract edit-history commands
- *(term)* group increment commands with selection editing
- *(term)* group surround commands with selection editing
- *(term)* group line insertion commands with insert-mode editing
- *(term)* extract mode-transition commands
- *(term)* extract insert-mode editing commands
- *(term)* extract register and clipboard commands
- *(term)* extract selection replacement and deletion commands
- *(term)* group indentation and comment commands with editing ([#38](https://github.com/mitos-editor/mitos/pull/38))
- *(term)* extract text transformation commands ([#37](https://github.com/mitos-editor/mitos/pull/37))
- *(term)* extract selection command handlers ([#36](https://github.com/mitos-editor/mitos/pull/36))
- *(term)* extract movement command handlers ([#35](https://github.com/mitos-editor/mitos/pull/35))
- *(term)* extract command-line orchestration ([#33](https://github.com/mitos-editor/mitos/pull/33))
- *(term)* extract typable command catalog ([#31](https://github.com/mitos-editor/mitos/pull/31))
- *(term)* extract static command catalog ([#30](https://github.com/mitos-editor/mitos/pull/30))
- *(term)* extract mappable command logic ([#28](https://github.com/mitos-editor/mitos/pull/28))
- *(term)* extract command context and callback adapters ([#27](https://github.com/mitos-editor/mitos/pull/27))
- editor and terminal configuration ownership ([#26](https://github.com/mitos-editor/mitos/pull/26))
- *(deps)* bump unicode-width from 0.1.12 to 0.2.0 ([#5](https://github.com/mitos-editor/mitos/pull/5))
- *(editor)* defer startup work through background jobs ([#17](https://github.com/mitos-editor/mitos/pull/17))
- improve crate and public API documentation
- *(paths)* separate display and operational paths
- *(tui)* own generic panel widgets
- *(ui)* move input primitives to ui-core
- extract snippets crate
- extract command-line crate
- *(ui)* consolidate picker preview state
- *(book)* refresh documentation and theme
- *(crates)* document workspace packages
- *(rust)* migrate workspace to edition 2024
- *(deps)* centralize workspace dependencies
- replace once_cell with std primitives
- *(ui)* always frame popup panels
- *(ui)* extract shared UI primitives
- *(commands)* mark typable commands cold
- *(diagnostics)* box owned strings
- outline Editor error and warning paths
- *(term)* extract shared shell command logic
- *(ui)* avoid cloning diagnostic text
- *(ui)* remove transient render allocations
- *(ui)* normalize documentation panels
- *(ui)* share Ratatui scrollbar rendering
- *(ui)* centralize panel chrome
- *(ui)* simplify remaining panel layouts
- *(ui)* render prompt completions as a table
- *(ui)* render buffer line as tabs
- *(ui)* lay out status rows with Ratatui
- *(ui)* use Ratatui picker layouts
- *(ui)* use Ratatui popup scrollbars
- *(ui)* use exact paragraph measurement
- *(ui)* centralize application layout
- rename packages, change default theme
