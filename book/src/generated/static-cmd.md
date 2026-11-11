| Name | Description | Default keybinds |
| --- | --- | --- |
| `no_op` | Do nothing |  |
| `move_char_left` | Move left | normal: `` h ``, `` <left> ``, insert: `` <left> `` |
| `move_char_right` | Move right | normal: `` l ``, `` <right> ``, insert: `` <right> `` |
| `move_line_up` | Move up | normal: `` gk `` |
| `move_line_down` | Move down | normal: `` gj `` |
| `move_visual_line_up` | Move up | normal: `` k ``, `` <up> ``, insert: `` <up> `` |
| `move_visual_line_down` | Move down | normal: `` j ``, `` <down> ``, insert: `` <down> `` |
| `extend_char_left` | Extend left | select: `` h ``, `` <left> `` |
| `extend_char_right` | Extend right | select: `` l ``, `` <right> `` |
| `extend_line_up` | Extend up | select: `` gk `` |
| `extend_line_down` | Extend down | select: `` gj `` |
| `extend_visual_line_up` | Extend up | select: `` k ``, `` <up> `` |
| `extend_visual_line_down` | Extend down | select: `` j ``, `` <down> `` |
| `copy_selection_on_next_line` | Copy selection on next line | normal: `` C ``, select: `` C `` |
| `copy_selection_on_previous_line` | Copy selection on previous line | normal: `` <A-C> ``, select: `` <A-C> `` |
| `move_next_word_start` | Move to start of next word | normal: `` w `` |
| `move_previous_word_start` | Move to start of previous word | normal: `` b `` |
| `move_next_word_end` | Move to end of next word | normal: `` e `` |
| `move_previous_word_end` | Move to end of previous word |  |
| `move_next_long_word_start` | Move to start of next long word | normal: `` W `` |
| `move_previous_long_word_start` | Move to start of previous long word | normal: `` B `` |
| `move_next_long_word_end` | Move to end of next long word | normal: `` E `` |
| `move_previous_long_word_end` | Move to end of previous long word |  |
| `move_next_sub_word_start` | Move to start of next sub word |  |
| `move_previous_sub_word_start` | Move to start of previous sub word |  |
| `move_next_sub_word_end` | Move to end of next sub word |  |
| `move_previous_sub_word_end` | Move to end of previous sub word |  |
| `move_parent_node_end` | Move to end of the parent node | normal: `` <A-e> `` |
| `move_parent_node_start` | Move to beginning of the parent node | normal: `` <A-b> `` |
| `extend_next_word_start` | Extend to start of next word | select: `` w `` |
| `extend_previous_word_start` | Extend to start of previous word | select: `` b `` |
| `extend_next_word_end` | Extend to end of next word | select: `` e `` |
| `extend_previous_word_end` | Extend to end of previous word |  |
| `extend_next_long_word_start` | Extend to start of next long word | select: `` W `` |
| `extend_previous_long_word_start` | Extend to start of previous long word | select: `` B `` |
| `extend_next_long_word_end` | Extend to end of next long word | select: `` E `` |
| `extend_previous_long_word_end` | Extend to end of prev long word |  |
| `extend_next_sub_word_start` | Extend to start of next sub word |  |
| `extend_previous_sub_word_start` | Extend to start of previous sub word |  |
| `extend_next_sub_word_end` | Extend to end of next sub word |  |
| `extend_previous_sub_word_end` | Extend to end of prev sub word |  |
| `extend_parent_node_end` | Extend to end of the parent node | select: `` <A-e> `` |
| `extend_parent_node_start` | Extend to beginning of the parent node | select: `` <A-b> `` |
| `find_till_next_char` | Move till next occurrence of char | normal: `` t `` |
| `find_next_char` | Move to next occurrence of char | normal: `` f `` |
| `extend_till_next_char` | Extend till next occurrence of char | select: `` t `` |
| `extend_next_char` | Extend to next occurrence of char | select: `` f `` |
| `find_till_previous_char` | Move till previous occurrence of char | normal: `` T `` |
| `find_previous_char` | Move to previous occurrence of char | normal: `` F `` |
| `extend_till_previous_char` | Extend till previous occurrence of char | select: `` T `` |
| `extend_previous_char` | Extend to previous occurrence of char | select: `` F `` |
| `repeat_last_motion` | Repeat last motion | normal: `` <A-.> ``, select: `` <A-.> `` |
| `replace_char` | Replace each selected grapheme with a character | normal: `` r ``, select: `` r `` |
| `replace` | Replace selections with entered text | normal: `` <A-r> ``, select: `` <A-r> `` |
| `switch_case` | Switch (toggle) case | normal: `` ~ ``, select: `` ~ `` |
| `switch_to_uppercase` | Switch to uppercase | normal: `` <A-`> ``, select: `` <A-`> `` |
| `switch_to_lowercase` | Switch to lowercase | normal: `` ` ``, select: `` ` `` |
| `page_up` | Move page up | normal: `` <C-b> ``, `` Z<C-b> ``, `` z<C-b> ``, `` <pageup> ``, `` Z<pageup> ``, `` z<pageup> ``, select: `` <C-b> ``, `` Z<C-b> ``, `` z<C-b> ``, `` <pageup> ``, `` Z<pageup> ``, `` z<pageup> ``, insert: `` <pageup> `` |
| `page_down` | Move page down | normal: `` <C-f> ``, `` Z<C-f> ``, `` z<C-f> ``, `` <pagedown> ``, `` Z<pagedown> ``, `` z<pagedown> ``, select: `` <C-f> ``, `` Z<C-f> ``, `` z<C-f> ``, `` <pagedown> ``, `` Z<pagedown> ``, `` z<pagedown> ``, insert: `` <pagedown> `` |
| `half_page_up` | Move half page up |  |
| `half_page_down` | Move half page down |  |
| `page_cursor_up` | Move page and cursor up |  |
| `page_cursor_down` | Move page and cursor down |  |
| `page_cursor_half_up` | Move page and cursor half up | normal: `` <C-u> ``, `` Z<C-u> ``, `` z<C-u> ``, `` Z<backspace> ``, `` z<backspace> ``, select: `` <C-u> ``, `` Z<C-u> ``, `` z<C-u> ``, `` Z<backspace> ``, `` z<backspace> `` |
| `page_cursor_half_down` | Move page and cursor half down | normal: `` <C-d> ``, `` Z<C-d> ``, `` z<C-d> ``, `` Z<space> ``, `` z<space> ``, select: `` <C-d> ``, `` Z<C-d> ``, `` z<C-d> ``, `` Z<space> ``, `` z<space> `` |
| `select_all` | Select whole document | normal: `` % ``, select: `` % `` |
| `select_regex` | Select all regex matches inside selections | normal: `` s ``, select: `` s `` |
| `split_selection` | Split selections on regex matches | normal: `` S ``, select: `` S `` |
| `split_selection_on_newline` | Split selection on newlines | normal: `` <A-s> ``, select: `` <A-s> `` |
| `merge_selections` | Merge selections | normal: `` <A-minus> ``, select: `` <A-minus> `` |
| `merge_consecutive_selections` | Merge consecutive selections | normal: `` <A-_> ``, select: `` <A-_> `` |
| `search_forward` | Search forward for regex pattern | normal: `` / ``, `` Z/ ``, `` z/ ``, select: `` / ``, `` Z/ ``, `` z/ `` |
| `search_backward` | Search backward for regex pattern | normal: `` ? ``, `` Z? ``, `` z? ``, select: `` ? ``, `` Z? ``, `` z? `` |
| `search_next` | Select next search match | normal: `` n ``, `` Zn ``, `` zn `` |
| `search_previous` | Select previous search match | normal: `` N ``, `` ZN ``, `` zN `` |
| `extend_search_next` | Add next search match to selection | select: `` n ``, `` Zn ``, `` zn `` |
| `extend_search_previous` | Add previous search match to selection | select: `` N ``, `` ZN ``, `` zN `` |
| `search_selection` | Use current selection as search pattern | normal: `` <A-*> ``, select: `` <A-*> `` |
| `search_selection_detect_word_boundaries` | Use current selection as the search pattern, automatically wrapping with `\b` on word boundaries | normal: `` * ``, select: `` * `` |
| `make_search_word_bounded` | Modify current search to make it word bounded |  |
| `search_picker` | Search text in the workspace | normal: `` <space>/ ``, select: `` <space>/ `` |
| `search_picker_in_current_working_directory` | Search text in the current working directory |  |
| `extend_line` | Select current line, if already selected, extend to another line based on the anchor |  |
| `extend_line_below` | Select current line, if already selected, extend to next line | normal: `` x ``, select: `` x `` |
| `extend_line_above` | Select current line, if already selected, extend to previous line |  |
| `select_line_above` | Select current line, if already selected, extend or shrink line above based on the anchor |  |
| `select_line_below` | Select current line, if already selected, extend or shrink line below based on the anchor |  |
| `extend_to_line_bounds` | Extend selection to line bounds | normal: `` X ``, select: `` X `` |
| `shrink_to_line_bounds` | Shrink selection to line bounds | normal: `` <A-x> ``, select: `` <A-x> `` |
| `delete_selection` | Delete selection | normal: `` d ``, select: `` d `` |
| `delete_selection_noyank` | Delete selection without yanking | normal: `` <A-d> ``, select: `` <A-d> `` |
| `change_selection` | Change selection | normal: `` c ``, select: `` c `` |
| `change_selection_noyank` | Change selection without yanking | normal: `` <A-c> ``, select: `` <A-c> `` |
| `fold_close` | Close the innermost syntax fold | normal: `` Zc ``, `` zc ``, select: `` Zc ``, `` zc `` |
| `fold_open` | Open the fold at each cursor | normal: `` Zo ``, `` zo ``, select: `` Zo ``, `` zo `` |
| `fold_toggle` | Toggle the fold at each cursor | normal: `` Za ``, `` za ``, select: `` Za ``, `` za `` |
| `fold_selection` | Fold selected lines | normal: `` Zf ``, `` zf ``, select: `` Zf ``, `` zf `` |
| `fold_all` | Close all syntax folds | normal: `` ZM ``, `` zM ``, select: `` ZM ``, `` zM `` |
| `unfold_all` | Open all folds | normal: `` ZR ``, `` zR ``, select: `` ZR ``, `` zR `` |
| `collapse_selection` | Collapse selection into single cursor | normal: `` ; ``, select: `` ; `` |
| `flip_selections` | Flip selection cursor and anchor | normal: `` <A-;> ``, select: `` <A-;> `` |
| `undo_selection` | Go to previous selection | normal: `` <A-u> ``, select: `` <A-u> `` |
| `redo_selection` | Go to next selection | normal: `` <A-U> ``, select: `` <A-U> `` |
| `ensure_selections_forward` | Ensure all selections face forward | normal: `` <A-:> ``, select: `` <A-:> `` |
| `insert_mode` | Insert before selection | normal: `` i ``, select: `` i `` |
| `append_mode` | Append after selection | normal: `` a ``, select: `` a `` |
| `command_mode` | Enter command mode | normal: `` : ``, select: `` : `` |
| `file_picker` | Open file picker | normal: `` <space>f ``, select: `` <space>f `` |
| `file_picker_in_current_buffer_directory` | Open file picker at current buffer's directory | normal: `` <space>F ``, select: `` <space>F `` |
| `file_picker_in_current_working_directory` | Open file picker at current working directory |  |
| `file_explorer` | Open file explorer in workspace root | normal: `` <space>e ``, select: `` <space>e `` |
| `file_explorer_in_current_buffer_directory` | Open file explorer at current buffer's directory | normal: `` <space>E ``, select: `` <space>E `` |
| `file_explorer_in_current_working_directory` | Open file explorer at current working directory |  |
| `code_action` | Perform code action | normal: `` <space>a ``, select: `` <space>a `` |
| `buffer_picker` | Open buffer picker | normal: `` <space>b ``, select: `` <space>b `` |
| `jumplist_picker` | Open jumplist picker | normal: `` <space>jj ``, select: `` <space>jj `` |
| `quicklist_picker` | Open quicklist picker | normal: `` <space>q ``, select: `` <space>q `` |
| `add_to_quicklist` | Add current location to quicklist | normal: `` <A-q> ``, select: `` <A-q> `` |
| `symbol_picker` | Open symbol picker |  |
| `syntax_symbol_picker` | Open symbol picker from syntax information |  |
| `lsp_or_syntax_symbol_picker` | Open symbol picker from LSP or syntax information | normal: `` <space>s ``, select: `` <space>s `` |
| `vcs_show_change` | Show hunk diff | normal: `` <space>gi ``, select: `` <space>gi `` |
| `vcs_goto_next_conflict` | Go to the next unresolved conflict | normal: `` ]m ``, select: `` ]m `` |
| `vcs_goto_previous_conflict` | Go to the previous unresolved conflict | normal: `` [m ``, select: `` [m `` |
| `vcs_blame_line` | Show line blame | normal: `` <space>gb ``, select: `` <space>gb `` |
| `vcs_toggle_inline_blame` | Toggle inline blame | normal: `` <space>gt ``, select: `` <space>gt `` |
| `vcs_history_picker` | Browse commits | normal: `` <space>gl ``, select: `` <space>gl `` |
| `vcs_file_history_picker` | Browse file history | normal: `` <space>gf ``, select: `` <space>gf `` |
| `vcs_stash_picker` | Browse stashes | normal: `` <space>gS ``, select: `` <space>gS `` |
| `vcs_changed_file_picker` | Browse changed files | normal: `` <space>gs ``, select: `` <space>gs `` |
| `vcs_diff` | Show file diff | normal: `` <space>gd ``, select: `` <space>gd `` |
| `vcs_diff_refresh` | Retry loading the current diff review buffer |  |
| `vcs_diff_open_source` | Jump from a diff review to its source location | normal: `` <ret> ``, select: `` <ret> `` |
| `vcs_diff_open_file` | Open the current file from a diff review |  |
| `vcs_changed_file_picker_in_repository` | Open changed file picker in repository |  |
| `select_references_to_symbol_under_cursor` | Select symbol references | normal: `` <space>h ``, select: `` <space>h `` |
| `workspace_symbol_picker` | Open workspace symbol picker |  |
| `syntax_workspace_symbol_picker` | Open workspace symbol picker from syntax information |  |
| `lsp_or_syntax_workspace_symbol_picker` | Open workspace symbol picker from LSP or syntax information | normal: `` <space>S ``, select: `` <space>S `` |
| `diagnostics_picker` | Open diagnostic picker | normal: `` <space>d ``, select: `` <space>d `` |
| `workspace_diagnostics_picker` | Open workspace diagnostic picker | normal: `` <space>D ``, select: `` <space>D `` |
| `last_picker` | Open last picker | normal: `` <space>' ``, select: `` <space>' `` |
| `insert_at_line_start` | Insert at start of line | normal: `` I ``, select: `` I `` |
| `insert_at_line_end` | Insert at end of line | normal: `` A ``, select: `` A `` |
| `open_below` | Open new line below selection | normal: `` o ``, select: `` o `` |
| `open_above` | Open new line above selection | normal: `` O ``, select: `` O `` |
| `normal_mode` | Enter normal mode | normal: `` <esc> ``, select: `` v ``, insert: `` <esc> `` |
| `select_mode` | Enter selection extend mode | normal: `` v `` |
| `exit_select_mode` | Exit selection mode | select: `` <esc> `` |
| `goto_definition` | Go to definition | normal: `` gd ``, `` <F12> ``, select: `` gd ``, `` <F12> `` |
| `goto_declaration` | Go to declaration | normal: `` gD ``, select: `` gD `` |
| `add_newline_above` | Add newline above | normal: `` [<space> ``, select: `` [<space> `` |
| `add_newline_below` | Add newline below | normal: `` ]<space> ``, select: `` ]<space> `` |
| `move_lines_up` | Move current line selection up | normal: `` <C-k> ``, select: `` <C-k> `` |
| `move_lines_down` | Move current line selection down | normal: `` <C-j> ``, select: `` <C-j> `` |
| `goto_type_definition` | Go to type definition | normal: `` gy ``, select: `` gy `` |
| `goto_implementation` | Go to implementation | normal: `` gi ``, select: `` gi `` |
| `show_code_lenses_under_cursor` | Show code lenses under cursor | normal: `` <space>l ``, select: `` <space>l `` |
| `goto_file_start` | Go to line number `<n>` else file start | normal: `` gg `` |
| `goto_file_end` | Go to file end |  |
| `extend_to_file_start` | Extend to line number `<n>` else file start | select: `` gg `` |
| `extend_to_file_end` | Extend to file end |  |
| `goto_file` | Go to files/URLs in selections | normal: `` gf ``, select: `` gf `` |
| `goto_file_in_horizontal_split` | Open files in selections in a horizontal split | normal: `` <C-w>f ``, select: `` <C-w>f `` |
| `goto_file_in_vertical_split` | Open files in selections in a vertical split | normal: `` <C-w>F ``, select: `` <C-w>F `` |
| `goto_reference` | Go to references | normal: `` gr ``, select: `` gr `` |
| `goto_window_top` | Go to window top | normal: `` gt ``, select: `` gt `` |
| `goto_window_center` | Go to window center | normal: `` gc ``, select: `` gc `` |
| `goto_window_bottom` | Go to window bottom | normal: `` gb ``, select: `` gb `` |
| `goto_last_accessed_file` | Go to last accessed file | normal: `` ga ``, select: `` ga `` |
| `goto_last_modified_file` | Go to last modified file | normal: `` gm ``, select: `` gm `` |
| `goto_last_modification` | Go to last modification | normal: `` g. ``, select: `` g. `` |
| `goto_line` | Go to the counted line, or the last line without a count; extend in select mode | normal: `` G ``, select: `` G `` |
| `goto_last_line` | Go to last line | normal: `` ge `` |
| `extend_to_last_line` | Extend to last line | select: `` ge `` |
| `goto_first_error` | Go to first error |  |
| `goto_last_error` | Go to last error |  |
| `goto_next_error` | Go to next error |  |
| `goto_previous_error` | Go to previous error |  |
| `goto_first_warning` | Go to first warning diagnostic |  |
| `goto_last_warning` | Go to last warning diagnostic |  |
| `goto_next_warning` | Go to next warning diagnostic |  |
| `goto_previous_warning` | Go to previous warning diagnostic |  |
| `goto_first_info` | Go to first info diagnostic |  |
| `goto_last_info` | Go to last info diagnostic |  |
| `goto_next_info` | Go to next info diagnostic |  |
| `goto_previous_info` | Go to previous info diagnostic |  |
| `goto_first_hint` | Go to first hint diagnostic |  |
| `goto_last_hint` | Go to last hint diagnostic |  |
| `goto_next_hint` | Go to next hint diagnostic |  |
| `goto_previous_hint` | Go to previous hint diagnostic |  |
| `goto_first_diagnostic` | Go to first diagnostic | normal: `` [D ``, select: `` [D `` |
| `goto_last_diagnostic` | Go to last diagnostic | normal: `` ]D ``, select: `` ]D `` |
| `goto_next_diagnostic` | Go to next diagnostic | normal: `` ]d ``, select: `` ]d `` |
| `goto_previous_diagnostic` | Go to previous diagnostic | normal: `` [d ``, select: `` [d `` |
| `goto_next_spelling` | Go to next spelling finding | normal: `` ]s ``, select: `` ]s `` |
| `goto_previous_spelling` | Go to previous spelling finding | normal: `` [s ``, select: `` [s `` |
| `goto_next_quicklist` | Go to next quicklist entry | normal: `` ]q ``, select: `` ]q `` |
| `goto_previous_quicklist` | Go to previous quicklist entry | normal: `` [q ``, select: `` [q `` |
| `goto_next_file_quicklist` | Go to next quicklist entry in current file | normal: `` ]l ``, select: `` ]l `` |
| `goto_previous_file_quicklist` | Go to previous quicklist entry in current file | normal: `` [l ``, select: `` [l `` |
| `vcs_goto_next_change` | Go to next change | normal: `` ]g ``, select: `` ]g `` |
| `vcs_goto_previous_change` | Go to previous change | normal: `` [g ``, select: `` [g `` |
| `vcs_goto_first_change` | Go to first change | normal: `` [G ``, select: `` [G `` |
| `vcs_goto_last_change` | Go to last change | normal: `` ]G ``, select: `` ]G `` |
| `goto_line_start` | Go to line start | normal: `` gh ``, `` <home> ``, select: `` gh ``, insert: `` <home> `` |
| `goto_line_end` | Go to line end | normal: `` gl ``, `` <end> ``, select: `` gl `` |
| `goto_column` | Go to column | normal: `` g\| `` |
| `extend_to_column` | Extend to column | select: `` g\| `` |
| `goto_next_buffer` | Go to next buffer | normal: `` gn ``, select: `` gn `` |
| `goto_previous_buffer` | Go to previous buffer | normal: `` gp ``, select: `` gp `` |
| `goto_line_end_newline` | Go to newline at line end | insert: `` <end> `` |
| `goto_first_nonwhitespace` | Go to first non-blank in line | normal: `` gs ``, select: `` gs `` |
| `trim_selections` | Trim whitespace from selections | normal: `` _ ``, select: `` _ `` |
| `extend_to_line_start` | Extend to line start | select: `` <home> `` |
| `extend_to_first_nonwhitespace` | Extend to first non-blank in line |  |
| `extend_to_line_end` | Extend to line end | select: `` <end> `` |
| `extend_to_line_end_newline` | Extend to line end |  |
| `signature_help` | Show signature help |  |
| `smart_tab` | Insert tab if all cursors have all whitespace to their left; otherwise, run a separate command. | insert: `` <tab> `` |
| `insert_tab` | Insert tab char | insert: `` <S-tab> `` |
| `insert_newline` | Insert newline char | insert: `` <C-j> ``, `` <ret> `` |
| `insert_char_interactive` | Insert an interactively-chosen char |  |
| `append_char_interactive` | Append an interactively-chosen char |  |
| `delete_char_backward` | Delete previous char | insert: `` <C-h> ``, `` <backspace> ``, `` <S-backspace> `` |
| `delete_char_forward` | Delete next char | insert: `` <C-d> ``, `` <del> `` |
| `delete_sub_word_backward` | Delete previous subword |  |
| `delete_sub_word_forward` | Delete next subword |  |
| `delete_word_backward` | Delete previous word | insert: `` <C-w> ``, `` <A-backspace> `` |
| `delete_word_forward` | Delete next word | insert: `` <A-d> ``, `` <A-del> `` |
| `kill_to_line_start` | Delete till start of line | insert: `` <C-u> `` |
| `kill_to_line_end` | Delete till end of line | insert: `` <C-k> `` |
| `undo` | Undo change | normal: `` u ``, select: `` u `` |
| `redo` | Redo change | normal: `` U ``, select: `` U `` |
| `earlier` | Move backward in history | normal: `` g<minus> ``, select: `` g<minus> `` |
| `later` | Move forward in history | normal: `` g+ ``, select: `` g+ `` |
| `commit_undo_checkpoint` | Start a new undo step without leaving insert mode | insert: `` <C-g>u `` |
| `yank` | Yank selection | normal: `` y ``, select: `` y `` |
| `yank_to_clipboard` | Yank selections to clipboard | normal: `` <space>y ``, select: `` <space>y `` |
| `yank_to_primary_clipboard` | Yank selections to primary clipboard |  |
| `yank_joined` | Join and yank selections |  |
| `yank_joined_to_clipboard` | Join and yank selections to clipboard |  |
| `yank_primary_selection_to_clipboard` | Yank primary selection to clipboard | normal: `` <space>Y ``, select: `` <space>Y `` |
| `yank_joined_to_primary_clipboard` | Join and yank selections to primary clipboard |  |
| `yank_primary_selection_to_primary_clipboard` | Yank primary selection to primary clipboard |  |
| `replace_with_yanked` | Replace with yanked text | normal: `` R ``, select: `` R `` |
| `replace_selections_with_clipboard` | Replace selections by clipboard content | normal: `` <space>R ``, select: `` <space>R `` |
| `replace_selections_with_primary_clipboard` | Replace selections by primary clipboard |  |
| `paste_after` | Paste after selection | normal: `` p ``, select: `` p `` |
| `paste_before` | Paste before selection | normal: `` P ``, select: `` P `` |
| `paste_after_all` | Paste every register value after each selection |  |
| `paste_before_all` | Paste every register value before each selection |  |
| `paste_clipboard_after` | Paste clipboard after selections | normal: `` <space>p ``, select: `` <space>p `` |
| `paste_clipboard_before` | Paste clipboard before selections | normal: `` <space>P ``, select: `` <space>P `` |
| `paste_primary_clipboard_after` | Paste primary clipboard after selections |  |
| `paste_primary_clipboard_before` | Paste primary clipboard before selections |  |
| `indent` | Indent selection | normal: `` <gt> ``, select: `` <gt> `` |
| `unindent` | Unindent selection | normal: `` <lt> ``, select: `` <lt> `` |
| `format_selections` | Format selection | normal: `` = ``, select: `` = `` |
| `join_lines` | Join lines inside selection | normal: `` J ``, select: `` J `` |
| `join_lines_and_select_spaces` | Join lines inside selection and select spaces | normal: `` <A-J> ``, select: `` <A-J> `` |
| `keep_selections` | Keep selections matching regex | normal: `` K ``, select: `` K `` |
| `remove_selections` | Remove selections matching regex | normal: `` <A-K> ``, select: `` <A-K> `` |
| `align_selections` | Align selections in column | normal: `` & ``, select: `` & `` |
| `keep_primary_selection` | Keep primary selection | normal: `` , ``, select: `` , `` |
| `remove_primary_selection` | Remove primary selection | normal: `` <A-,> ``, select: `` <A-,> `` |
| `completion` | Invoke completion popup | insert: `` <C-x> `` |
| `inline_completion_accept` | Accept inline completion |  |
| `inline_completion_dismiss` | Dismiss inline completion |  |
| `inline_completion_next` | Cycle to next inline completion |  |
| `inline_completion_previous` | Cycle to previous inline completion |  |
| `inline_completion_trigger` | Trigger inline completion |  |
| `hover` | Show docs for item under cursor | normal: `` <space>k ``, select: `` <space>k `` |
| `toggle_comments` | Comment/uncomment selections | normal: `` <C-c> ``, `` <space>c ``, select: `` <C-c> ``, `` <space>c `` |
| `toggle_line_comments` | Line comment/uncomment selections | normal: `` <space><A-c> ``, select: `` <space><A-c> `` |
| `toggle_block_comments` | Block comment/uncomment selections | normal: `` <space>C ``, select: `` <space>C `` |
| `cycle_primary_selection_forward` | Make the next selection primary | normal: `` ) ``, select: `` ) `` |
| `cycle_primary_selection_backward` | Make the previous selection primary | normal: `` ( ``, select: `` ( `` |
| `rotate_selection_contents_forward` | Rotate selection contents forward | normal: `` <A-)> ``, select: `` <A-)> `` |
| `rotate_selection_contents_backward` | Rotate selections contents backward | normal: `` <A-(> ``, select: `` <A-(> `` |
| `reverse_selection_contents` | Reverse selections contents |  |
| `expand_selection` | Expand selection to parent syntax node | normal: `` <A-o> ``, `` <A-up> ``, select: `` <A-o> ``, `` <A-up> `` |
| `shrink_selection` | Shrink selection to previously expanded syntax node | normal: `` <A-i> ``, `` <A-down> ``, select: `` <A-i> ``, `` <A-down> `` |
| `select_next_sibling` | Select next sibling in the syntax tree | normal: `` <A-n> ``, `` <A-right> ``, select: `` <A-n> ``, `` <A-right> `` |
| `select_previous_sibling` | Select previous sibling in the syntax tree | normal: `` <A-p> ``, `` <A-left> ``, select: `` <A-p> ``, `` <A-left> `` |
| `select_all_siblings` | Select all siblings of the current node | normal: `` <A-a> ``, select: `` <A-a> `` |
| `select_all_children` | Select all children of the current node | normal: `` <A-I> ``, `` <S-A-down> ``, select: `` <A-I> ``, `` <S-A-down> `` |
| `jump_forward` | Jump forward on jumplist | normal: `` <C-i> ``, `` <tab> ``, select: `` <C-i> ``, `` <tab> `` |
| `jump_backward` | Jump backward on jumplist | normal: `` <C-o> ``, select: `` <C-o> `` |
| `save_selection` | Record current selections in the jumplist | normal: `` <space>js ``, select: `` <space>js `` |
| `jump_view_right` | Jump to right split | normal: `` <C-w>l ``, `` <C-w><C-l> ``, `` <C-w><right> ``, select: `` <C-w>l ``, `` <C-w><C-l> ``, `` <C-w><right> `` |
| `jump_view_left` | Jump to left split | normal: `` <C-w>h ``, `` <C-w><C-h> ``, `` <C-w><left> ``, select: `` <C-w>h ``, `` <C-w><C-h> ``, `` <C-w><left> `` |
| `toggle_sidebar` | Toggle file explorer sidebar | normal: `` <C-w>b ``, select: `` <C-w>b `` |
| `focus_sidebar` | Open and focus file explorer sidebar | normal: `` <C-w>e ``, select: `` <C-w>e `` |
| `switch_sidebar_focus` | Switch focus between editor and file explorer sidebar | normal: `` <C-e> ``, select: `` <C-e> `` |
| `jump_view_up` | Jump to split above | normal: `` <C-w>k ``, `` <C-w><up> ``, `` <C-w><C-k> ``, select: `` <C-w>k ``, `` <C-w><up> ``, `` <C-w><C-k> `` |
| `jump_view_down` | Jump to split below | normal: `` <C-w>j ``, `` <C-w><C-j> ``, `` <C-w><down> ``, select: `` <C-w>j ``, `` <C-w><C-j> ``, `` <C-w><down> `` |
| `swap_view_right` | Swap with right split | normal: `` <C-w>L ``, `` <C-w><S-right> ``, select: `` <C-w>L ``, `` <C-w><S-right> `` |
| `swap_view_left` | Swap with left split | normal: `` <C-w>H ``, `` <C-w><S-left> ``, select: `` <C-w>H ``, `` <C-w><S-left> `` |
| `swap_view_up` | Swap with split above | normal: `` <C-w>K ``, `` <C-w><S-up> ``, select: `` <C-w>K ``, `` <C-w><S-up> `` |
| `swap_view_down` | Swap with split below | normal: `` <C-w>J ``, `` <C-w><S-down> ``, select: `` <C-w>J ``, `` <C-w><S-down> `` |
| `transpose_view` | Transpose splits | normal: `` <C-w>t ``, `` <C-w><C-t> ``, select: `` <C-w>t ``, `` <C-w><C-t> `` |
| `resize_mode` | Resize windows (Enter to confirm, Escape to cancel) | normal: `` <C-w>r ``, select: `` <C-w>r `` |
| `resize_view_left` | Resize window left |  |
| `resize_view_right` | Resize window right |  |
| `resize_view_up` | Resize window up |  |
| `resize_view_down` | Resize window down |  |
| `grow_view_width` | Grow split width |  |
| `shrink_view_width` | Shrink split width |  |
| `grow_view_height` | Grow split height |  |
| `shrink_view_height` | Shrink split height |  |
| `equalize_views` | Make all splits the same size | normal: `` <C-w>= ``, select: `` <C-w>= `` |
| `rotate_view` | Go to next window | normal: `` <C-w>w ``, `` <C-w><C-w> ``, select: `` <C-w>w ``, `` <C-w><C-w> `` |
| `rotate_view_reverse` | Go to previous window | normal: `` <C-w>W ``, select: `` <C-w>W `` |
| `split_horizontal` | Horizontal bottom split | normal: `` <C-w>s ``, `` <C-w><C-s> ``, select: `` <C-w>s ``, `` <C-w><C-s> `` |
| `split_horizontal_new` | Horizontal bottom split scratch buffer | normal: `` <C-w>ns ``, `` <C-w>n<C-s> ``, select: `` <C-w>ns ``, `` <C-w>n<C-s> `` |
| `split_vertical` | Vertical right split | normal: `` <C-w>v ``, `` <C-w><C-v> ``, select: `` <C-w>v ``, `` <C-w><C-v> `` |
| `split_vertical_new` | Vertical right split scratch buffer | normal: `` <C-w>nv ``, `` <C-w>n<C-v> ``, select: `` <C-w>nv ``, `` <C-w>n<C-v> `` |
| `close_view` | Close current view | normal: `` <C-w>q ``, `` <C-w><C-q> ``, select: `` <C-w>q ``, `` <C-w><C-q> `` |
| `close_other_views` | Close views except current | normal: `` <C-w>o ``, `` <C-w><C-o> ``, select: `` <C-w>o ``, `` <C-w><C-o> `` |
| `select_register` | Select register | normal: `` " ``, select: `` " `` |
| `insert_register` | Insert register | insert: `` <C-r> `` |
| `copy_between_registers` | Copy between two registers |  |
| `center_view_horizontally` | Scroll horizontally to center the primary cursor | normal: `` Zm ``, `` zm ``, select: `` Zm ``, `` zm `` |
| `align_view_top` | Align view top | normal: `` Zt ``, `` zt ``, select: `` Zt ``, `` zt `` |
| `center_view_vertically` | Scroll vertically to center the primary cursor | normal: `` Zz ``, `` zz ``, select: `` Zz ``, `` zz `` |
| `align_view_bottom` | Align view bottom | normal: `` Zb ``, `` zb ``, select: `` Zb ``, `` zb `` |
| `scroll_up` | Scroll view up | normal: `` Zk ``, `` zk ``, `` Z<up> ``, `` z<up> ``, select: `` Zk ``, `` zk ``, `` Z<up> ``, `` z<up> `` |
| `scroll_down` | Scroll view down | normal: `` Zj ``, `` zj ``, `` Z<down> ``, `` z<down> ``, select: `` Zj ``, `` zj ``, `` Z<down> ``, `` z<down> `` |
| `match_brackets` | Go to matching bracket | normal: `` mm ``, select: `` mm `` |
| `surround_add` | Surround add | normal: `` ms ``, select: `` ms `` |
| `surround_replace` | Surround replace | normal: `` mr ``, select: `` mr `` |
| `surround_delete` | Surround delete | normal: `` md ``, select: `` md `` |
| `select_textobject_around` | Select around object | normal: `` ma ``, select: `` ma `` |
| `select_textobject_inner` | Select inside object | normal: `` mi ``, select: `` mi `` |
| `select_all_textobjects_around` | Select all textobjects around selections | normal: `` mA ``, select: `` mA `` |
| `select_all_textobjects_inner` | Select all textobjects inside selections | normal: `` mI ``, select: `` mI `` |
| `goto_next_function` | Go to next function | normal: `` ]f ``, select: `` ]f `` |
| `goto_previous_function` | Go to previous function | normal: `` [f ``, select: `` [f `` |
| `goto_next_class` | Go to next type definition | normal: `` ]t ``, select: `` ]t `` |
| `goto_previous_class` | Go to previous type definition | normal: `` [t ``, select: `` [t `` |
| `goto_next_parameter` | Go to next parameter | normal: `` ]a ``, select: `` ]a `` |
| `goto_previous_parameter` | Go to previous parameter | normal: `` [a ``, select: `` [a `` |
| `goto_next_comment` | Go to next comment | normal: `` ]c ``, select: `` ]c `` |
| `goto_previous_comment` | Go to previous comment | normal: `` [c ``, select: `` [c `` |
| `goto_next_test` | Go to next test | normal: `` ]T ``, select: `` ]T `` |
| `goto_previous_test` | Go to previous test | normal: `` [T ``, select: `` [T `` |
| `goto_next_xml_element` | Go to next (X)HTML element | normal: `` ]x ``, select: `` ]x `` |
| `goto_previous_xml_element` | Go to previous (X)HTML element | normal: `` [x ``, select: `` [x `` |
| `goto_next_entry` | Go to next pairing | normal: `` ]e ``, select: `` ]e `` |
| `goto_previous_entry` | Go to previous pairing | normal: `` [e ``, select: `` [e `` |
| `goto_next_paragraph` | Go to next paragraph | normal: `` ]p ``, select: `` ]p `` |
| `goto_previous_paragraph` | Go to previous paragraph | normal: `` [p ``, select: `` [p `` |
| `debug_launch_picker` | Choose a launch configuration and start debugging | normal: `` <space>Xl ``, `` <space>xl ``, select: `` <space>Xl ``, `` <space>xl `` |
| `debug_restart` | Restart debugging session | normal: `` <space>Xr ``, `` <space>xr ``, select: `` <space>Xr ``, `` <space>xr `` |
| `debug_toggle_breakpoints` | Toggle breakpoints | normal: `` <F9> ``, `` <space>Xb ``, `` <space>xb ``, select: `` <F9> ``, `` <space>Xb ``, `` <space>xb `` |
| `debug_continue` | Continue program execution | normal: `` <F5> ``, `` <space>Xc ``, `` <space>xc ``, select: `` <F5> ``, `` <space>Xc ``, `` <space>xc `` |
| `debug_pause` | Pause program execution | normal: `` <space>Xh ``, `` <space>xh ``, select: `` <space>Xh ``, `` <space>xh `` |
| `debug_step_in` | Step in | normal: `` <F11> ``, `` <space>Xi ``, `` <space>xi ``, select: `` <F11> ``, `` <space>Xi ``, `` <space>xi `` |
| `debug_step_out` | Step out | normal: `` <S-F11> ``, `` <space>Xo ``, `` <space>xo ``, select: `` <S-F11> ``, `` <space>Xo ``, `` <space>xo `` |
| `debug_step_over` | Step over the current statement | normal: `` <F10> ``, `` <space>Xn ``, `` <space>xn ``, select: `` <F10> ``, `` <space>Xn ``, `` <space>xn `` |
| `debug_variables` | Browse variables | normal: `` <space>Xv ``, `` <space>xv ``, select: `` <space>Xv ``, `` <space>xv `` |
| `debug_terminate` | End debug session | normal: `` <space>Xt ``, `` <space>xt ``, select: `` <space>Xt ``, `` <space>xt `` |
| `debug_edit_breakpoint_condition` | Edit breakpoint condition on current line | normal: `` <space>XBc ``, `` <space>xBc ``, select: `` <space>XBc ``, `` <space>xBc `` |
| `debug_edit_breakpoint_log_message` | Edit breakpoint log message on current line | normal: `` <space>XBl ``, `` <space>xBl ``, select: `` <space>XBl ``, `` <space>xBl `` |
| `debug_session_picker` | Open debug session picker | normal: `` <space>Xss ``, `` <space>xss ``, select: `` <space>Xss ``, `` <space>xss `` |
| `debug_thread_picker` | Choose the current debug thread | normal: `` <space>Xst ``, `` <space>xst ``, select: `` <space>Xst ``, `` <space>xst `` |
| `debug_stack_frame_picker` | Open debug stack frame picker | normal: `` <space>Xsf ``, `` <space>xsf ``, select: `` <space>Xsf ``, `` <space>xsf `` |
| `debug_enable_exception_breakpoints` | Enable exception breakpoints | normal: `` <space>Xe ``, `` <space>xe ``, select: `` <space>Xe ``, `` <space>xe `` |
| `debug_disable_exception_breakpoints` | Disable exception breakpoints | normal: `` <space>XE ``, `` <space>xE ``, select: `` <space>XE ``, `` <space>xE `` |
| `shell_pipe` | Pipe selections through shell command | normal: `` \| ``, select: `` \| `` |
| `shell_pipe_to` | Pipe selections into shell command ignoring output | normal: `` <A-\|> ``, select: `` <A-\|> `` |
| `shell_insert_output` | Insert shell command output before selections | normal: `` ! ``, select: `` ! `` |
| `shell_append_output` | Append shell command output after selections | normal: `` <A-!> ``, select: `` <A-!> `` |
| `shell_keep_pipe` | Filter selections with shell predicate | normal: `` $ ``, select: `` $ `` |
| `suspend` | Suspend and return to shell | normal: `` <C-z> ``, select: `` <C-z> `` |
| `rename_symbol` | Rename symbol | normal: `` <F2> ``, `` <space>r ``, select: `` <F2> ``, `` <space>r `` |
| `increment` | Increment item under cursor | normal: `` <C-a> ``, select: `` <C-a> `` |
| `decrement` | Decrement item under cursor | normal: `` <C-x> ``, select: `` <C-x> `` |
| `record_macro` | Record macro | normal: `` Q ``, select: `` Q `` |
| `replay_macro` | Replay macro | normal: `` q ``, select: `` q `` |
| `command_palette` | Open command palette | normal: `` <F1> ``, `` <space>? ``, select: `` <F1> ``, `` <space>? ``, insert: `` <F1> `` |
| `goto_word` | Jump to a two-character label | normal: `` gw `` |
| `extend_to_word` | Extend to a two-character label | select: `` gw `` |
| `snippet_picker` | Choose and insert a user-defined snippet |  |
| `snippet_next_choice` | Choose the next snippet placeholder value |  |
| `snippet_previous_choice` | Choose the previous snippet placeholder value |  |
| `snippet_next_tabstop` | Go to next snippet placeholder |  |
| `snippet_previous_tabstop` | Go to previous snippet placeholder |  |
| `make_first_selection_primary` | Make the first selection your primary one |  |
| `make_last_selection_primary` | Make the last selection your primary one |  |
