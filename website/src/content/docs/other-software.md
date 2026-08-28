---
title: Mitos mode in other software
description: Find Mitos-style editing modes for editors, shells, and other tools.
---

## Compatible editing modes in other software

Mitos inherits Helix's keymap and interaction model. Existing Helix-mode
integrations may therefore be useful to Mitos users, although these external
projects are Helix-branded and are not maintained by Mitos.

## Other editors

| Editor | Plugin or feature providing Helix editing | Comments |
| --- | --- | --- |
| [Vim](https://www.vim.org/) | [helix.vim](https://github.com/chtenb/helix.vim) config | |
| [IntelliJ IDEA](https://www.jetbrains.com/idea/) / [Android Studio](https://developer.android.com/studio) | [IdeaVim](https://plugins.jetbrains.com/plugin/164-ideavim) plugin + [helix.idea.vim](https://github.com/chtenb/helix.vim) config | Minimum recommended version is IdeaVim 2.19.0. |
| [Visual Studio](https://visualstudio.microsoft.com/) | [VsVim](https://marketplace.visualstudio.com/items?itemName=JaredParMSFT.VsVim) plugin + [helix.vs.vim](https://github.com/chtenb/helix.vim) config | |
| [Visual Studio Code](https://code.visualstudio.com/) | [Dance](https://marketplace.visualstudio.com/items?itemName=gregoire.dance) extension, or its [Helix fork](https://marketplace.visualstudio.com/items?itemName=kend.dancehelixkey) | The Helix fork has diverged. |
| [Visual Studio Code](https://code.visualstudio.com/) | [Helix for VS Code](https://marketplace.visualstudio.com/items?itemName=jasew.vscode-helix-emulation) extension | |
| [Zed](https://zed.dev/) | Native via keybindings ([issue](https://github.com/zed-industries/zed/issues/4642)) | |
| [CodeMirror](https://codemirror.net/) | [codemirror-helix](https://gitlab.com/_rvidal/codemirror-helix) | |
| [Lite XL](https://lite-xl.com/) | [lite-modal-hx](https://codeberg.org/Mandarancio/lite-modal-hx) | |

## Shells

| Shell | Plugin or feature providing Helix editing |
| --- | --- |
| Fish | [Feature request](https://github.com/fish-shell/fish-shell/issues/7748) |
| Fish | [fish-helix](https://github.com/sshilovsky/fish-helix/tree/main) |
| Zsh | [helix-zsh](https://github.com/john-h-k/helix-zsh) or [zsh-helix-mode](https://github.com/Multirious/zsh-helix-mode) |
| Nushell | [Feature request](https://github.com/nushell/reedline/issues/639) |

## Other software

| Software | Plugin or feature providing Helix editing | Comments |
| --- | --- | --- |
| [Obsidian](https://obsidian.md/) | [Obsidian-Helix](https://github.com/Sinono3/obsidian-helix) | Uses `codemirror-helix` listed above. |
