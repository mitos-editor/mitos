# Quicklist

The quicklist keeps a picker's matched file locations available after the picker closes.
Unlike the [jumplist](./jumplist.md), which records locations you leave, the quicklist is an explicit result set you can traverse in either direction.

## Populating the quicklist

While a picker containing files or locations is open, press `Ctrl-q` to copy its current matched entries into the quicklist.
The entries keep their current picker order, and the picker remains open.
For example, you can filter global-search, diagnostics, or symbol results and capture only those matches.

The quicklist is editor-global and temporary.
Capturing entries from another picker replaces its contents; capturing a picker with no matching locations clears it.

## Navigating entries

| Key | Action |
| --- | --- |
| `]q` | Jump to the next entry, wrapping at the end |
| `[q` | Jump to the previous entry, wrapping at the beginning |
| `]l` | Jump to the next entry in the current file or unsaved buffer |
| `[l` | Jump to the previous entry in the current file or unsaved buffer |
| `Space q` | Open the quicklist picker |

The navigation commands accept a count, so `3]q` jumps forward three entries.
Quicklist jumps are added to the jumplist, allowing `Ctrl-o` to return to the location before the jump.

The quicklist picker shows every captured entry and marks the current one with `*`.
Selecting an entry opens it and makes it the current quicklist position.

## Typical workflow

1. Open a picker that shows file locations, such as global search, diagnostics, symbols, or another location-based picker.
2. Filter the picker until it contains the set of locations you want.
3. Press `Ctrl-q` to copy the current matched items into the quicklist.
4. Close the picker, then move through all collected locations with `]q` and `[q`.
5. Use `]l` and `[l` to restrict navigation to the current file, or `Space q` to browse the complete list.
