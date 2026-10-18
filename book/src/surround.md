# Surround

Add, replace, or remove matching delimiters around selected text, such as
parentheses, brackets, and quotes.

![Surround demo](https://user-images.githubusercontent.com/23398472/122865801-97073180-d344-11eb-8142-8f43809982c6.gif)

| Key Sequence                      | Action                                  |
| --------------------------------- | --------------------------------------- |
| `ms<char>` (after selecting text) | Add surround characters to selection    |
| `mr<char_to_replace><new_char>`   | Replace the closest surround characters |
| `md<char_to_delete>`              | Delete the closest surround characters  |

You can use counts to act on outer pairs.

Surround can also act on multiple selections. For example, to change every occurrence of `(use)` to `[use]`:

1. `%` to select the whole file
2. `s` to select matches of a search term
3. Input `use` and hit Enter
4. `mr([` to replace the parentheses with square brackets

Surround operations accept single delimiter characters.
