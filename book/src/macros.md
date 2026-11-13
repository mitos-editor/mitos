# Macros and undo

## Repeat an edit

Press `.` to repeat the last insertion or prompted replacement on your current
selections. For example, change a word with `c`, type its replacement, press
`Esc`, then select another word and press `.`.

Use a macro when you want to repeat several commands together.
Use multiple selections when the same edit should apply to every match in a
region. If each match needs a decision, search with `/` or `*`, use `n` to review
matches, and repeat the edit with `.` only where you want it. See the
[worked examples](./editing-effectively/repeated-edits.md#review-repeated-matches-one-at-a-time).

## Record and replay a macro

1. Press `Q` in normal mode to start recording.
2. Perform the commands you want to repeat.
3. Press `Q` again to stop recording.
4. Press `q` to replay the recording, or `3q` to replay it three times.

For example, start at the beginning of a line, press `Q`, then `I`, type
`TODO: `, press `Esc`, move to the next line with `j`, and press `Q` again.
Each replay inserts the prefix and moves to the following line.

Record into a named [register](./registers.md) to keep more than one macro:
`"aQ` starts recording into register `a`; `Q` stops it; `"aq` replays it.
Without an explicit register, recording and replay use `@`.

Macros are stored as text containing key sequences. They share registers with
yanked text, so yanking into a macro's register replaces that recording. To
keep a sequence in your configuration, use a
[macro keybinding](./remapping.md), such as `"@miw"`.

## Undo and redo

Press `u` to undo and `U` to redo. Text entered during an insert-mode session
normally forms one undo step when you return to normal mode. Press `Ctrl-g u`
while inserting to start a new undo step without leaving insert mode.

Mitos also keeps earlier edit branches after an undo followed by a new edit.
Use `g-` and `g+` to move backward and forward through edit history.
The commands `:earlier 5` and `:later 5` move by a number of steps;
`:earlier 1m` moves back by a time span.

`Alt-u` and `Alt-Shift-u` undo and redo [selection changes](./usage.md#undo-selection-changes).
They do not undo text edits.
