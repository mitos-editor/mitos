---
title: Usage
description: Learn Mitos' modes, selections, buffers, and movement model.
---

For a full interactive introduction to Mitos, refer to the
[tutor](https://github.com/mitos-editor/mitos/blob/main/runtime/tutor) which
can be accessed via the command `ms --tutor` or `:tutor`.

> 💡 Currently, not all functionality is fully documented, please refer to the
> [key mappings](../keymap/) list.

## Modes

Mitos is a modal editor, meaning it has different modes for different tasks. The main modes are:

* [Normal mode](../keymap/#normal-mode): For navigation and editing commands. This is the default mode.
* [Insert mode](../keymap/#insert-mode): For typing text directly into the document. Access by typing `i` in normal mode.
* [Select/extend mode](../keymap/#select--extend-mode): For making selections and performing operations on them. Access by typing `v` in normal mode.

## Buffers

Buffers are in-memory representations of files. You can have multiple buffers open at once. Use [pickers](../pickers/) or commands like `:buffer-next` and `:buffer-previous` to open buffers or switch between them.

## Selection-first editing

Inspired by [Kakoune](http://kakoune.org/), Mitos follows the `selection → action` model. This means that whatever you are going to act on (a word, a paragraph, a line, etc.) is selected first and the action itself (delete, change, yank, etc.) comes second. A cursor is simply a single width selection.

## Multiple selections

Also inspired by Kakoune, multiple selections are a core mode of interaction in Mitos. For example, the standard way of replacing multiple instances of a word is to first select all instances (so there is one selection per instance) and then use the change action (`c`) to edit them all at the same time.

## Motions

Motions are commands that move the cursor or modify selections. They're used for navigation and text manipulation. Examples include `w` to move to the next word, or `f` to find a character. See the [Movement](../keymap/#movement) section of the keymap for more motions.
