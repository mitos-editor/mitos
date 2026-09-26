//! Static and typable command metadata, handler mappings, and alias lookup.

use std::{collections::HashMap, sync::LazyLock};

use ::command_line::{Args, Flag, Signature};

use super::{mappable::MappableCommand, typed};
use crate::{
    compositor,
    ui::{
        completers::{self, Completer},
        PromptEvent,
    },
};

macro_rules! static_commands {
    ( $($name:ident => $handler:path, $doc:literal,)* ) => {
        $(
            #[allow(non_upper_case_globals)]
            pub const $name: Self = Self::Static {
                name: stringify!($name),
                fun: $handler,
                doc: $doc
            };
        )*

        pub const STATIC_COMMAND_LIST: &'static [Self] = &[
            $( Self::$name, )*
        ];
    }
}

impl MappableCommand {
    #[rustfmt::skip]
    static_commands!(
        no_op => super::no_op, "Do nothing",
        move_char_left => super::movement::move_char_left, "Move left",
        move_char_right => super::movement::move_char_right, "Move right",
        move_line_up => super::movement::move_line_up, "Move up",
        move_line_down => super::movement::move_line_down, "Move down",
        move_visual_line_up => super::movement::move_visual_line_up, "Move up",
        move_visual_line_down => super::movement::move_visual_line_down, "Move down",
        extend_char_left => super::movement::extend_char_left, "Extend left",
        extend_char_right => super::movement::extend_char_right, "Extend right",
        extend_line_up => super::movement::extend_line_up, "Extend up",
        extend_line_down => super::movement::extend_line_down, "Extend down",
        extend_visual_line_up => super::movement::extend_visual_line_up, "Extend up",
        extend_visual_line_down => super::movement::extend_visual_line_down, "Extend down",
        copy_selection_on_next_line => super::selection::copy_selection_on_next_line, "Copy selection on next line",
        copy_selection_on_prev_line => super::selection::copy_selection_on_prev_line, "Copy selection on previous line",
        move_next_word_start => super::movement::move_next_word_start, "Move to start of next word",
        move_prev_word_start => super::movement::move_prev_word_start, "Move to start of previous word",
        move_next_word_end => super::movement::move_next_word_end, "Move to end of next word",
        move_prev_word_end => super::movement::move_prev_word_end, "Move to end of previous word",
        move_next_long_word_start => super::movement::move_next_long_word_start, "Move to start of next long word",
        move_prev_long_word_start => super::movement::move_prev_long_word_start, "Move to start of previous long word",
        move_next_long_word_end => super::movement::move_next_long_word_end, "Move to end of next long word",
        move_prev_long_word_end => super::movement::move_prev_long_word_end, "Move to end of previous long word",
        move_next_sub_word_start => super::movement::move_next_sub_word_start, "Move to start of next sub word",
        move_prev_sub_word_start => super::movement::move_prev_sub_word_start, "Move to start of previous sub word",
        move_next_sub_word_end => super::movement::move_next_sub_word_end, "Move to end of next sub word",
        move_prev_sub_word_end => super::movement::move_prev_sub_word_end, "Move to end of previous sub word",
        move_parent_node_end => super::move_parent_node_end, "Move to end of the parent node",
        move_parent_node_start => super::move_parent_node_start, "Move to beginning of the parent node",
        extend_next_word_start => super::movement::extend_next_word_start, "Extend to start of next word",
        extend_prev_word_start => super::movement::extend_prev_word_start, "Extend to start of previous word",
        extend_next_word_end => super::movement::extend_next_word_end, "Extend to end of next word",
        extend_prev_word_end => super::movement::extend_prev_word_end, "Extend to end of previous word",
        extend_next_long_word_start => super::movement::extend_next_long_word_start, "Extend to start of next long word",
        extend_prev_long_word_start => super::movement::extend_prev_long_word_start, "Extend to start of previous long word",
        extend_next_long_word_end => super::movement::extend_next_long_word_end, "Extend to end of next long word",
        extend_prev_long_word_end => super::movement::extend_prev_long_word_end, "Extend to end of prev long word",
        extend_next_sub_word_start => super::movement::extend_next_sub_word_start, "Extend to start of next sub word",
        extend_prev_sub_word_start => super::movement::extend_prev_sub_word_start, "Extend to start of previous sub word",
        extend_next_sub_word_end => super::movement::extend_next_sub_word_end, "Extend to end of next sub word",
        extend_prev_sub_word_end => super::movement::extend_prev_sub_word_end, "Extend to end of prev sub word",
        extend_parent_node_end => super::extend_parent_node_end, "Extend to end of the parent node",
        extend_parent_node_start => super::extend_parent_node_start, "Extend to beginning of the parent node",
        find_till_char => super::movement::find_till_char, "Move till next occurrence of char",
        find_next_char => super::movement::find_next_char, "Move to next occurrence of char",
        extend_till_char => super::movement::extend_till_char, "Extend till next occurrence of char",
        extend_next_char => super::movement::extend_next_char, "Extend to next occurrence of char",
        till_prev_char => super::movement::till_prev_char, "Move till previous occurrence of char",
        find_prev_char => super::movement::find_prev_char, "Move to previous occurrence of char",
        extend_till_prev_char => super::movement::extend_till_prev_char, "Extend till previous occurrence of char",
        extend_prev_char => super::movement::extend_prev_char, "Extend to previous occurrence of char",
        repeat_last_motion => super::movement::repeat_last_motion, "Repeat last motion",
        replace_char => super::editing::replace_char, "Replace each selected grapheme with a character",
        replace => super::editing::replace, "Replace selections with entered text",
        switch_case => super::editing::switch_case, "Switch (toggle) case",
        switch_to_uppercase => super::editing::switch_to_uppercase, "Switch to uppercase",
        switch_to_lowercase => super::editing::switch_to_lowercase, "Switch to lowercase",
        page_up => super::movement::page_up, "Move page up",
        page_down => super::movement::page_down, "Move page down",
        half_page_up => super::movement::half_page_up, "Move half page up",
        half_page_down => super::movement::half_page_down, "Move half page down",
        page_cursor_up => super::movement::page_cursor_up, "Move page and cursor up",
        page_cursor_down => super::movement::page_cursor_down, "Move page and cursor down",
        page_cursor_half_up => super::movement::page_cursor_half_up, "Move page and cursor half up",
        page_cursor_half_down => super::movement::page_cursor_half_down, "Move page and cursor half down",
        select_all => super::selection::select_all, "Select whole document",
        select_regex => super::selection::select_regex, "Select all regex matches inside selections",
        split_selection => super::selection::split_selection, "Split selections on regex matches",
        split_selection_on_newline => super::selection::split_selection_on_newline, "Split selection on newlines",
        merge_selections => super::selection::merge_selections, "Merge selections",
        merge_consecutive_selections => super::selection::merge_consecutive_selections, "Merge consecutive selections",
        search => super::search, "Search for regex pattern",
        rsearch => super::rsearch, "Reverse search for regex pattern",
        search_next => super::search_next, "Select next search match",
        search_prev => super::search_prev, "Select previous search match",
        extend_search_next => super::extend_search_next, "Add next search match to selection",
        extend_search_prev => super::extend_search_prev, "Add previous search match to selection",
        search_selection => super::search_selection, "Use current selection as search pattern",
        search_selection_detect_word_boundaries => super::search_selection_detect_word_boundaries, "Use current selection as the search pattern, automatically wrapping with `\\b` on word boundaries",
        make_search_word_bounded => super::make_search_word_bounded, "Modify current search to make it word bounded",
        global_search => super::global_search, "Global search in workspace folder",
        extend_line => super::selection::extend_line, "Select current line, if already selected, extend to another line based on the anchor",
        extend_line_below => super::selection::extend_line_below, "Select current line, if already selected, extend to next line",
        extend_line_above => super::selection::extend_line_above, "Select current line, if already selected, extend to previous line",
        select_line_above => super::selection::select_line_above, "Select current line, if already selected, extend or shrink line above based on the anchor",
        select_line_below => super::selection::select_line_below, "Select current line, if already selected, extend or shrink line below based on the anchor",
        extend_to_line_bounds => super::selection::extend_to_line_bounds, "Extend selection to line bounds",
        shrink_to_line_bounds => super::selection::shrink_to_line_bounds, "Shrink selection to line bounds",
        delete_selection => super::editing::delete_selection, "Delete selection",
        delete_selection_noyank => super::editing::delete_selection_noyank, "Delete selection without yanking",
        change_selection => super::editing::change_selection, "Change selection",
        change_selection_noyank => super::editing::change_selection_noyank, "Change selection without yanking",
        collapse_selection => super::selection::collapse_selection, "Collapse selection into single cursor",
        flip_selections => super::selection::flip_selections, "Flip selection cursor and anchor",
        ensure_selections_forward => super::selection::ensure_selections_forward, "Ensure all selections face forward",
        insert_mode => super::mode::insert_mode, "Insert before selection",
        append_mode => super::mode::append_mode, "Append after selection",
        command_mode => super::command_line::command_mode, "Enter command mode",
        file_picker => super::file_picker, "Open file picker",
        file_picker_in_current_buffer_directory => super::file_picker_in_current_buffer_directory, "Open file picker at current buffer's directory",
        file_picker_in_current_directory => super::file_picker_in_current_directory, "Open file picker at current working directory",
        file_explorer => super::file_explorer, "Open file explorer in workspace root",
        file_explorer_in_current_buffer_directory => super::file_explorer_in_current_buffer_directory, "Open file explorer at current buffer's directory",
        file_explorer_in_current_directory => super::file_explorer_in_current_directory, "Open file explorer at current working directory",
        code_action => super::lsp::code_action, "Perform code action",
        buffer_picker => super::buffer_picker, "Open buffer picker",
        jumplist_picker => super::jumplist_picker, "Open jumplist picker",
        quicklist_picker => super::quicklist_picker, "Open quicklist picker",
        symbol_picker => super::lsp::symbol_picker, "Open symbol picker",
        syntax_symbol_picker => super::syntax::syntax_symbol_picker, "Open symbol picker from syntax information",
        lsp_or_syntax_symbol_picker => super::lsp_or_syntax_symbol_picker, "Open symbol picker from LSP or syntax information",
        changed_file_picker => super::changed_file_picker, "Open changed file picker in workspace",
        changed_file_picker_in_repository => super::changed_file_picker_in_repository, "Open changed file picker in repository",
        select_references_to_symbol_under_cursor => super::lsp::select_references_to_symbol_under_cursor, "Select symbol references",
        workspace_symbol_picker => super::lsp::workspace_symbol_picker, "Open workspace symbol picker",
        syntax_workspace_symbol_picker => super::syntax::syntax_workspace_symbol_picker, "Open workspace symbol picker from syntax information",
        lsp_or_syntax_workspace_symbol_picker => super::lsp_or_syntax_workspace_symbol_picker, "Open workspace symbol picker from LSP or syntax information",
        diagnostics_picker => super::lsp::diagnostics_picker, "Open diagnostic picker",
        workspace_diagnostics_picker => super::lsp::workspace_diagnostics_picker, "Open workspace diagnostic picker",
        last_picker => super::last_picker, "Open last picker",
        insert_at_line_start => super::insert::insert_at_line_start, "Insert at start of line",
        insert_at_line_end => super::insert::insert_at_line_end, "Insert at end of line",
        open_below => super::insert::open_below, "Open new line below selection",
        open_above => super::insert::open_above, "Open new line above selection",
        normal_mode => super::mode::normal_mode, "Enter normal mode",
        select_mode => super::mode::select_mode, "Enter selection extend mode",
        exit_select_mode => super::mode::exit_select_mode, "Exit selection mode",
        goto_definition => super::lsp::goto_definition, "Goto definition",
        goto_declaration => super::lsp::goto_declaration, "Goto declaration",
        add_newline_above => super::insert::add_newline_above, "Add newline above",
        add_newline_below => super::insert::add_newline_below, "Add newline below",
        goto_type_definition => super::lsp::goto_type_definition, "Goto type definition",
        goto_implementation => super::lsp::goto_implementation, "Goto implementation",
        goto_file_start => super::goto_file_start, "Goto line number `<n>` else file start",
        goto_file_end => super::goto_file_end, "Goto file end",
        extend_to_file_start => super::extend_to_file_start, "Extend to line number `<n>` else file start",
        extend_to_file_end => super::extend_to_file_end, "Extend to file end",
        goto_file => super::goto_file, "Goto files/URLs in selections",
        goto_file_hsplit => super::goto_file_hsplit, "Goto files in selections (hsplit)",
        goto_file_vsplit => super::goto_file_vsplit, "Goto files in selections (vsplit)",
        goto_reference => super::lsp::goto_reference, "Goto references",
        goto_window_top => super::movement::goto_window_top, "Goto window top",
        goto_window_center => super::movement::goto_window_center, "Goto window center",
        goto_window_bottom => super::movement::goto_window_bottom, "Goto window bottom",
        goto_last_accessed_file => super::goto_last_accessed_file, "Goto last accessed file",
        goto_last_modified_file => super::goto_last_modified_file, "Goto last modified file",
        goto_last_modification => super::goto_last_modification, "Goto last modification",
        goto_line => super::goto_line, "Goto line",
        goto_last_line => super::goto_last_line, "Goto last line",
        extend_to_last_line => super::extend_to_last_line, "Extend to last line",
        goto_first_diag => super::goto_first_diag, "Goto first diagnostic",
        goto_last_diag => super::goto_last_diag, "Goto last diagnostic",
        goto_next_diag => super::goto_next_diag, "Goto next diagnostic",
        goto_prev_diag => super::goto_prev_diag, "Goto previous diagnostic",
        goto_next_spelling => super::goto_next_spelling, "Goto next spelling finding",
        goto_prev_spelling => super::goto_prev_spelling, "Goto previous spelling finding",
        goto_next_quicklist => super::goto_next_quicklist, "Goto next quicklist entry",
        goto_prev_quicklist => super::goto_prev_quicklist, "Goto previous quicklist entry",
        goto_next_file_quicklist => super::goto_next_file_quicklist, "Goto next quicklist entry in current file",
        goto_prev_file_quicklist => super::goto_prev_file_quicklist, "Goto previous quicklist entry in current file",
        goto_next_change => super::goto_next_change, "Goto next change",
        goto_prev_change => super::goto_prev_change, "Goto previous change",
        goto_first_change => super::goto_first_change, "Goto first change",
        goto_last_change => super::goto_last_change, "Goto last change",
        goto_line_start => super::movement::goto_line_start, "Goto line start",
        goto_line_end => super::movement::goto_line_end, "Goto line end",
        goto_column => super::goto_column, "Goto column",
        extend_to_column => super::extend_to_column, "Extend to column",
        goto_next_buffer => super::goto_next_buffer, "Goto next buffer",
        goto_previous_buffer => super::goto_previous_buffer, "Goto previous buffer",
        goto_line_end_newline => super::movement::goto_line_end_newline, "Goto newline at line end",
        goto_first_nonwhitespace => super::movement::goto_first_nonwhitespace, "Goto first non-blank in line",
        trim_selections => super::selection::trim_selections, "Trim whitespace from selections",
        extend_to_line_start => super::movement::extend_to_line_start, "Extend to line start",
        extend_to_first_nonwhitespace => super::movement::extend_to_first_nonwhitespace, "Extend to first non-blank in line",
        extend_to_line_end => super::movement::extend_to_line_end, "Extend to line end",
        extend_to_line_end_newline => super::movement::extend_to_line_end_newline, "Extend to line end",
        signature_help => super::lsp::signature_help, "Show signature help",
        smart_tab => super::insert::smart_tab, "Insert tab if all cursors have all whitespace to their left; otherwise, run a separate command.",
        insert_tab => super::insert::insert_tab, "Insert tab char",
        insert_newline => super::insert::insert_newline, "Insert newline char",
        insert_char_interactive => super::insert::insert_char_interactive, "Insert an interactively-chosen char",
        append_char_interactive => super::insert::append_char_interactive, "Append an interactively-chosen char",
        delete_char_backward => super::insert::delete_char_backward, "Delete previous char",
        delete_char_forward => super::insert::delete_char_forward, "Delete next char",
        delete_word_backward => super::insert::delete_word_backward, "Delete previous word",
        delete_word_forward => super::insert::delete_word_forward, "Delete next word",
        kill_to_line_start => super::insert::kill_to_line_start, "Delete till start of line",
        kill_to_line_end => super::insert::kill_to_line_end, "Delete till end of line",
        undo => super::history::undo, "Undo change",
        redo => super::history::redo, "Redo change",
        earlier => super::history::earlier, "Move backward in history",
        later => super::history::later, "Move forward in history",
        commit_undo_checkpoint => super::history::commit_undo_checkpoint, "Commit changes to new checkpoint",
        yank => super::registers::yank, "Yank selection",
        yank_to_clipboard => super::registers::yank_to_clipboard, "Yank selections to clipboard",
        yank_to_primary_clipboard => super::registers::yank_to_primary_clipboard, "Yank selections to primary clipboard",
        yank_joined => super::registers::yank_joined, "Join and yank selections",
        yank_joined_to_clipboard => super::registers::yank_joined_to_clipboard, "Join and yank selections to clipboard",
        yank_main_selection_to_clipboard => super::registers::yank_main_selection_to_clipboard, "Yank main selection to clipboard",
        yank_joined_to_primary_clipboard => super::registers::yank_joined_to_primary_clipboard, "Join and yank selections to primary clipboard",
        yank_main_selection_to_primary_clipboard => super::registers::yank_main_selection_to_primary_clipboard, "Yank main selection to primary clipboard",
        replace_with_yanked => super::registers::replace_with_yanked, "Replace with yanked text",
        replace_selections_with_clipboard => super::registers::replace_selections_with_clipboard, "Replace selections by clipboard content",
        replace_selections_with_primary_clipboard => super::registers::replace_selections_with_primary_clipboard, "Replace selections by primary clipboard",
        paste_after => super::registers::paste_after, "Paste after selection",
        paste_before => super::registers::paste_before, "Paste before selection",
        paste_clipboard_after => super::registers::paste_clipboard_after, "Paste clipboard after selections",
        paste_clipboard_before => super::registers::paste_clipboard_before, "Paste clipboard before selections",
        paste_primary_clipboard_after => super::registers::paste_primary_clipboard_after, "Paste primary clipboard after selections",
        paste_primary_clipboard_before => super::registers::paste_primary_clipboard_before, "Paste primary clipboard before selections",
        indent => super::editing::indent, "Indent selection",
        unindent => super::editing::unindent, "Unindent selection",
        format_selections => super::formatting::format_selections, "Format selection",
        join_selections => super::editing::join_selections, "Join lines inside selection",
        join_selections_space => super::editing::join_selections_space, "Join lines inside selection and select spaces",
        keep_selections => super::selection::keep_selections, "Keep selections matching regex",
        remove_selections => super::selection::remove_selections, "Remove selections matching regex",
        align_selections => super::editing::align_selections, "Align selections in column",
        keep_primary_selection => super::selection::keep_primary_selection, "Keep primary selection",
        remove_primary_selection => super::selection::remove_primary_selection, "Remove primary selection",
        completion => super::completion, "Invoke completion popup",
        hover => super::lsp::hover, "Show docs for item under cursor",
        toggle_comments => super::editing::toggle_comments, "Comment/uncomment selections",
        toggle_line_comments => super::editing::toggle_line_comments, "Line comment/uncomment selections",
        toggle_block_comments => super::editing::toggle_block_comments, "Block comment/uncomment selections",
        rotate_selections_forward => super::selection::rotate_selections_forward, "Rotate selections forward",
        rotate_selections_backward => super::selection::rotate_selections_backward, "Rotate selections backward",
        rotate_selection_contents_forward => super::editing::rotate_selection_contents_forward, "Rotate selection contents forward",
        rotate_selection_contents_backward => super::editing::rotate_selection_contents_backward, "Rotate selections contents backward",
        reverse_selection_contents => super::editing::reverse_selection_contents, "Reverse selections contents",
        expand_selection => super::expand_selection, "Expand selection to parent syntax node",
        shrink_selection => super::shrink_selection, "Shrink selection to previously expanded syntax node",
        select_next_sibling => super::select_next_sibling, "Select next sibling in the syntax tree",
        select_prev_sibling => super::select_prev_sibling, "Select previous sibling the in syntax tree",
        select_all_siblings => super::select_all_siblings, "Select all siblings of the current node",
        select_all_children => super::select_all_children, "Select all children of the current node",
        jump_forward => super::jump_forward, "Jump forward on jumplist",
        jump_backward => super::jump_backward, "Jump backward on jumplist",
        save_selection => super::save_selection, "Save current selection to jumplist",
        jump_view_right => super::jump_view_right, "Jump to right split",
        jump_view_left => super::jump_view_left, "Jump to left split",
        jump_view_up => super::jump_view_up, "Jump to split above",
        jump_view_down => super::jump_view_down, "Jump to split below",
        swap_view_right => super::swap_view_right, "Swap with right split",
        swap_view_left => super::swap_view_left, "Swap with left split",
        swap_view_up => super::swap_view_up, "Swap with split above",
        swap_view_down => super::swap_view_down, "Swap with split below",
        transpose_view => super::transpose_view, "Transpose splits",
        rotate_view => super::rotate_view, "Goto next window",
        rotate_view_reverse => super::rotate_view_reverse, "Goto previous window",
        hsplit => super::hsplit, "Horizontal bottom split",
        hsplit_new => super::hsplit_new, "Horizontal bottom split scratch buffer",
        vsplit => super::vsplit, "Vertical right split",
        vsplit_new => super::vsplit_new, "Vertical right split scratch buffer",
        wclose => super::wclose, "Close window",
        wonly => super::wonly, "Close windows except current",
        select_register => super::registers::select_register, "Select register",
        insert_register => super::registers::insert_register, "Insert register",
        copy_between_registers => super::registers::copy_between_registers, "Copy between two registers",
        align_view_middle => super::movement::align_view_middle, "Align view middle",
        align_view_top => super::movement::align_view_top, "Align view top",
        align_view_center => super::movement::align_view_center, "Align view center",
        align_view_bottom => super::movement::align_view_bottom, "Align view bottom",
        scroll_up => super::movement::scroll_up, "Scroll view up",
        scroll_down => super::movement::scroll_down, "Scroll view down",
        match_brackets => super::match_brackets, "Goto matching bracket",
        surround_add => super::editing::surround_add, "Surround add",
        surround_replace => super::editing::surround_replace, "Surround replace",
        surround_delete => super::editing::surround_delete, "Surround delete",
        select_textobject_around => super::select_textobject_around, "Select around object",
        select_textobject_inner => super::select_textobject_inner, "Select inside object",
        select_all_textobjects_around => super::select_all_textobjects_around, "Select all textobjects around selections",
        select_all_textobjects_inner => super::select_all_textobjects_inner, "Select all textobjects inside selections",
        goto_next_function => super::goto_next_function, "Goto next function",
        goto_prev_function => super::goto_prev_function, "Goto previous function",
        goto_next_class => super::goto_next_class, "Goto next type definition",
        goto_prev_class => super::goto_prev_class, "Goto previous type definition",
        goto_next_parameter => super::goto_next_parameter, "Goto next parameter",
        goto_prev_parameter => super::goto_prev_parameter, "Goto previous parameter",
        goto_next_comment => super::goto_next_comment, "Goto next comment",
        goto_prev_comment => super::goto_prev_comment, "Goto previous comment",
        goto_next_test => super::goto_next_test, "Goto next test",
        goto_prev_test => super::goto_prev_test, "Goto previous test",
        goto_next_xml_element => super::goto_next_xml_element, "Goto next (X)HTML element",
        goto_prev_xml_element => super::goto_prev_xml_element, "Goto previous (X)HTML element",
        goto_next_entry => super::goto_next_entry, "Goto next pairing",
        goto_prev_entry => super::goto_prev_entry, "Goto previous pairing",
        goto_next_paragraph => super::movement::goto_next_paragraph, "Goto next paragraph",
        goto_prev_paragraph => super::movement::goto_prev_paragraph, "Goto previous paragraph",
        dap_launch => super::dap::dap_launch, "Launch debug target",
        dap_restart => super::dap::dap_restart, "Restart debugging session",
        dap_toggle_breakpoints => super::dap::dap_toggle_breakpoints, "Toggle breakpoints",
        dap_continue => super::dap::dap_continue, "Continue program execution",
        dap_pause => super::dap::dap_pause, "Pause program execution",
        dap_step_in => super::dap::dap_step_in, "Step in",
        dap_step_out => super::dap::dap_step_out, "Step out",
        dap_next => super::dap::dap_next, "Step to next",
        dap_variables => super::dap::dap_variables, "List variables",
        dap_terminate => super::dap::dap_terminate, "End debug session",
        dap_edit_condition => super::dap::dap_edit_condition, "Edit breakpoint condition on current line",
        dap_edit_log => super::dap::dap_edit_log, "Edit breakpoint log message on current line",
        dap_switch_thread => super::dap::dap_switch_thread, "Switch current thread",
        dap_switch_stack_frame => super::dap::dap_switch_stack_frame, "Switch stack frame",
        dap_enable_exceptions => super::dap::dap_enable_exceptions, "Enable exception breakpoints",
        dap_disable_exceptions => super::dap::dap_disable_exceptions, "Disable exception breakpoints",
        shell_pipe => super::shell::shell_pipe, "Pipe selections through shell command",
        shell_pipe_to => super::shell::shell_pipe_to, "Pipe selections into shell command ignoring output",
        shell_insert_output => super::shell::shell_insert_output, "Insert shell command output before selections",
        shell_append_output => super::shell::shell_append_output, "Append shell command output after selections",
        shell_keep_pipe => super::shell::shell_keep_pipe, "Filter selections with shell predicate",
        suspend => super::suspend, "Suspend and return to shell",
        rename_symbol => super::lsp::rename_symbol, "Rename symbol",
        increment => super::editing::increment, "Increment item under cursor",
        decrement => super::editing::decrement, "Decrement item under cursor",
        record_macro => super::record_macro, "Record macro",
        replay_macro => super::replay_macro, "Replay macro",
        command_palette => super::command_palette, "Open command palette",
        goto_word => super::goto_word, "Jump to a two-character label",
        extend_to_word => super::extend_to_word, "Extend to a two-character label",
        goto_next_tabstop => super::goto_next_tabstop, "Goto next snippet placeholder",
        goto_prev_tabstop => super::goto_prev_tabstop, "Goto next snippet placeholder",
        rotate_selections_first => super::selection::rotate_selections_first, "Make the first selection your primary one",
        rotate_selections_last => super::selection::rotate_selections_last, "Make the last selection your primary one",
    );
}

#[derive(Clone)]
pub struct TypableCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub doc: &'static str,
    // params, flags, helper, completer
    pub fun: fn(&mut compositor::Context, Args, PromptEvent) -> anyhow::Result<()>,
    /// What completion methods, if any, does this command have?
    pub completer: CommandCompleter,
    pub signature: Signature,
}

#[derive(Clone)]
pub struct CommandCompleter {
    // Arguments with specific completion methods based on their position.
    positional_args: &'static [Completer],

    // All remaining arguments will use this completion method, if set.
    var_args: Completer,
}

impl CommandCompleter {
    const fn none() -> Self {
        Self {
            positional_args: &[],
            var_args: completers::none,
        }
    }

    const fn positional(completers: &'static [Completer]) -> Self {
        Self {
            positional_args: completers,
            var_args: completers::none,
        }
    }

    const fn all(completer: Completer) -> Self {
        Self {
            positional_args: &[],
            var_args: completer,
        }
    }

    pub(super) fn for_argument_number(&self, n: usize) -> &Completer {
        match self.positional_args.get(n) {
            Some(completer) => completer,
            _ => &self.var_args,
        }
    }
}

/// This command accepts a single boolean --skip-visible flag and no positionals.
const BUFFER_CLOSE_OTHERS_SIGNATURE: Signature = Signature {
    positionals: (0, Some(0)),
    flags: &[Flag {
        name: "skip-visible",
        alias: Some('s'),
        doc: "don't close buffers that are visible",
        ..Flag::DEFAULT
    }],
    ..Signature::DEFAULT
};

// TODO: SHELL_SIGNATURE should specify var args for arguments, so that just completers::filename can be used,
// but Signature does not yet allow for var args.

/// This command handles all of its input as-is with no quoting or flags.
pub const SHELL_SIGNATURE: Signature = Signature {
    positionals: (1, Some(2)),
    raw_after: Some(1),
    ..Signature::DEFAULT
};

pub const SHELL_COMPLETER: CommandCompleter = CommandCompleter::positional(&[
    // Command name
    completers::program,
    // Shell argument(s)
    completers::repeating_filenames,
]);

pub(super) const WRITE_NO_FORMAT_FLAG: Flag = Flag {
    name: "no-format",
    doc: "skip auto-formatting",
    ..Flag::DEFAULT
};

pub(super) const WRITE_NO_CODE_ACTIONS_FLAG: Flag = Flag {
    name: "no-code-actions",
    doc: "skip code actions on save",
    ..Flag::DEFAULT
};

pub const TYPABLE_COMMAND_LIST: &[TypableCommand] = &[
    TypableCommand {
        name: "exit",
        aliases: &["x", "xit"],
        doc: "Write changes to disk if the buffer is modified and then quit. Accepts an optional path (`:exit some/path.txt`).",
        fun: typed::exit,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "exit!",
        aliases: &["x!", "xit!"],
        doc: "Force write changes to disk, creating necessary subdirectories, if the buffer is modified and then quit. Accepts an optional path (`:exit! some/path.txt`).",
        fun: typed::force_exit,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "quit",
        aliases: &["q"],
        doc: "Close the current view.",
        fun: typed::quit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "quit!",
        aliases: &["q!"],
        doc: "Force close the current view, ignoring unsaved changes.",
        fun: typed::force_quit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "open",
        aliases: &["o", "edit", "e"],
        doc: "Open a file from disk into the current view.",
        fun: typed::open,
        completer: CommandCompleter::all(completers::filename),
        signature: Signature {
            positionals: (1, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-close",
        aliases: &["bc", "bclose"],
        doc: "Close the current buffer.",
        fun: typed::buffer_close,
        completer: CommandCompleter::all(completers::buffer),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-close!",
        aliases: &["bc!", "bclose!"],
        doc: "Close the current buffer forcefully, ignoring unsaved changes.",
        fun: typed::force_buffer_close,
        completer: CommandCompleter::all(completers::buffer),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-close-others",
        aliases: &["bco", "bcloseother"],
        doc: "Close all buffers but the currently focused one.",
        fun: typed::buffer_close_others,
        completer: CommandCompleter::none(),
        signature: BUFFER_CLOSE_OTHERS_SIGNATURE,
    },
    TypableCommand {
        name: "buffer-close-others!",
        aliases: &["bco!", "bcloseother!"],
        doc: "Force close all buffers but the currently focused one.",
        fun: typed::force_buffer_close_others,
        completer: CommandCompleter::none(),
        signature: BUFFER_CLOSE_OTHERS_SIGNATURE,
    },
    TypableCommand {
        name: "buffer-close-all",
        aliases: &["bca", "bcloseall"],
        doc: "Close all buffers without quitting.",
        fun: typed::buffer_close_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-close-all!",
        aliases: &["bca!", "bcloseall!"],
        doc: "Force close all buffers ignoring unsaved changes without quitting.",
        fun: typed::force_buffer_close_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-next",
        aliases: &["bn", "bnext"],
        doc: "Goto next buffer.",
        fun: typed::buffer_next,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "buffer-previous",
        aliases: &["bp", "bprev"],
        doc: "Goto previous buffer.",
        fun: typed::buffer_previous,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write",
        aliases: &["w"],
        doc: "Write changes to disk. Accepts an optional path (`:write some/path.txt`).",
        fun: typed::write,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write!",
        aliases: &["w!"],
        doc: "Force write changes to disk creating necessary subdirectories. Accepts an optional path (`:write! some/path.txt`).",
        fun: typed::force_write,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG,WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-buffer-close",
        aliases: &["wbc"],
        doc: "Write changes to disk and closes the buffer. Accepts an optional path (`:write-buffer-close some/path.txt`).",
        fun: typed::write_buffer_close,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG,WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-buffer-close!",
        aliases: &["wbc!"],
        doc: "Force write changes to disk creating necessary subdirectories and closes the buffer. Accepts an optional path (`:write-buffer-close! some/path.txt`).",
        fun: typed::force_write_buffer_close,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG,WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "new",
        aliases: &["n"],
        doc: "Create a new scratch buffer.",
        fun: typed::new_file,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "format",
        aliases: &["fmt"],
        doc: "Format the file using an external formatter or language server.",
        fun: typed::format,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "indent-style",
        aliases: &[],
        doc: "Set the indentation style for editing. ('t' for tabs or 1-16 for number of spaces.)",
        fun: typed::set_indent_style,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "line-ending",
        aliases: &[],
        #[cfg(not(feature = "unicode-lines"))]
        doc: "Set the document's default line ending. Options: crlf, lf.",
        #[cfg(feature = "unicode-lines")]
        doc: "Set the document's default line ending. Options: crlf, lf, cr, ff, nel.",
        fun: typed::set_line_ending,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "earlier",
        aliases: &["ear"],
        doc: "Jump back to an earlier point in edit history. Accepts a number of steps or a time span.",
        fun: typed::earlier,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "later",
        aliases: &["lat"],
        doc: "Jump to a later point in edit history. Accepts a number of steps or a time span.",
        fun: typed::later,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-quit",
        aliases: &["wq"],
        doc: "Write changes to disk and close the current view. Accepts an optional path (`:wq some/path.txt`).",
        fun: typed::write_quit,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-quit!",
        aliases: &["wq!"],
        doc: "Write changes to disk and close the current view forcefully. Accepts an optional path (`:wq! some/path.txt`).",
        fun: typed::force_write_quit,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (0, Some(1)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-all",
        aliases: &["wa"],
        doc: "Write changes from all buffers to disk.",
        fun: typed::write_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-all!",
        aliases: &["wa!"],
        doc: "Forcefully write changes from all buffers to disk creating necessary subdirectories.",
        fun: typed::force_write_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-quit-all",
        aliases: &["wqa", "xa"],
        doc: "Write changes from all buffers to disk and close all views.",
        fun: typed::write_all_quit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "write-quit-all!",
        aliases: &["wqa!", "xa!"],
        doc: "Forcefully write changes from all buffers to disk, creating necessary subdirectories, and close all views (ignoring unsaved changes).",
        fun: typed::force_write_all_quit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[WRITE_NO_FORMAT_FLAG, WRITE_NO_CODE_ACTIONS_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "quit-all",
        aliases: &["qa"],
        doc: "Close all views.",
        fun: typed::quit_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "quit-all!",
        aliases: &["qa!"],
        doc: "Force close all views ignoring unsaved changes.",
        fun: typed::force_quit_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "cquit",
        aliases: &["cq"],
        doc: "Quit with exit code (default 1). Accepts an optional integer exit code (`:cq 2`).",
        fun: typed::cquit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "cquit!",
        aliases: &["cq!"],
        doc: "Force quit with exit code (default 1) ignoring unsaved changes. Accepts an optional integer exit code (`:cq! 2`).",
        fun: typed::force_cquit,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "theme",
        aliases: &[],
        doc: "Change the editor theme (show current theme if no name specified).",
        fun: typed::theme,
        completer: CommandCompleter::positional(&[completers::theme]),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "yank-join",
        aliases: &[],
        doc: "Yank joined selections. A separator can be provided as first argument. Default value is newline.",
        fun: typed::yank_joined,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clipboard-yank",
        aliases: &[],
        doc: "Yank main selection into system clipboard.",
        fun: typed::yank_main_selection_to_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clipboard-yank-join",
        aliases: &[],
        doc: "Yank joined selections into system clipboard. A separator can be provided as first argument. Default value is newline.", // FIXME: current UI can't display long doc.
        fun: typed::yank_joined_to_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "primary-clipboard-yank",
        aliases: &[],
        doc: "Yank main selection into system primary clipboard.",
        fun: typed::yank_main_selection_to_primary_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "primary-clipboard-yank-join",
        aliases: &[],
        doc: "Yank joined selections into system primary clipboard. A separator can be provided as first argument. Default value is newline.", // FIXME: current UI can't display long doc.
        fun: typed::yank_joined_to_primary_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clipboard-paste-after",
        aliases: &[],
        doc: "Paste system clipboard after selections.",
        fun: typed::paste_clipboard_after,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clipboard-paste-before",
        aliases: &[],
        doc: "Paste system clipboard before selections.",
        fun: typed::paste_clipboard_before,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clipboard-paste-replace",
        aliases: &[],
        doc: "Replace selections with content of system clipboard.",
        fun: typed::replace_selections_with_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "primary-clipboard-paste-after",
        aliases: &[],
        doc: "Paste primary clipboard after selections.",
        fun: typed::paste_primary_clipboard_after,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "primary-clipboard-paste-before",
        aliases: &[],
        doc: "Paste primary clipboard before selections.",
        fun: typed::paste_primary_clipboard_before,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "primary-clipboard-paste-replace",
        aliases: &[],
        doc: "Replace selections with content of system primary clipboard.",
        fun: typed::replace_selections_with_primary_clipboard,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "show-clipboard-provider",
        aliases: &[],
        doc: "Show clipboard provider name in status bar.",
        fun: typed::show_clipboard_provider,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "change-current-directory",
        aliases: &["cd"],
        doc: "Change the current working directory.",
        fun: typed::change_current_directory,
        completer: CommandCompleter::positional(&[completers::directory]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "show-directory-stack",
        aliases: &[],
        doc: "Show the directory stack as a space-delimited string.",
        fun: typed::show_directory_stack,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "push-directory",
        aliases: &["pushd"],
        doc: "Save and then change the current directory.",
        fun: typed::push_directory,
        completer: CommandCompleter::positional(&[completers::directory]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "pop-directory",
        aliases: &["popd"],
        doc: "Remove the top entry from the directory stack, and cd to the new top directory..",
        fun: typed::pop_directory,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "show-directory",
        aliases: &["pwd"],
        doc: "Show the current working directory.",
        fun: typed::show_current_directory,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "encoding",
        aliases: &[],
        doc: "Set encoding. Based on `https://encoding.spec.whatwg.org`.",
        fun: typed::set_encoding,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "character-info",
        aliases: &["char"],
        doc: "Get info about the character under the primary cursor.",
        fun: typed::get_character_info,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "reload",
        aliases: &["rl"],
        doc: "Discard changes and reload from the source file.",
        fun: typed::reload,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "reload-all",
        aliases: &["rla"],
        doc: "Discard changes and reload all documents from the source files.",
        fun: typed::reload_all,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "update",
        aliases: &["u"],
        doc: "Write changes only if the file has been modified.",
        fun: typed::update,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[WRITE_NO_FORMAT_FLAG],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "lsp-workspace-command",
        aliases: &[],
        doc: "Open workspace command picker",
        fun: typed::lsp_workspace_command,
        completer: CommandCompleter::positional(&[completers::lsp_workspace_command]),
        signature: Signature {
            positionals: (0, None),
            raw_after: Some(1),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "lsp-restart",
        aliases: &[],
        doc: "Restarts the given language servers, or all language servers that are used by the current file if no arguments are supplied",
        fun: typed::lsp_restart,
        completer: CommandCompleter::all(completers::configured_language_servers),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "lsp-stop",
        aliases: &[],
        doc: "Stops the given language servers, or all language servers that are used by the current file if no arguments are supplied",
        fun: typed::lsp_stop,
        completer: CommandCompleter::all(completers::active_language_servers),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "tree-sitter-scopes",
        aliases: &[],
        doc: "Display tree sitter scopes, primarily for theming and development.",
        fun: typed::tree_sitter_scopes,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "tree-sitter-highlight-name",
        aliases: &[],
        doc: "Display name of tree-sitter highlight scope under the cursor.",
        fun: typed::tree_sitter_highlight_name,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "tree-sitter-layers",
        aliases: &[],
        doc: "Display language names of tree-sitter injection layers under the cursor.",
        fun: typed::tree_sitter_layers,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "debug-start",
        aliases: &["dbg"],
        doc: "Start a debug session from a given template with given parameters.",
        fun: typed::debug_start,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "debug-remote",
        aliases: &["dbg-tcp"],
        doc: "Connect to a debug adapter by TCP address and start a debugging session from a given template with given parameters.",
        fun: typed::debug_remote,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "debug-eval",
        aliases: &[],
        doc: "Evaluate expression in current debug context.",
        fun: typed::debug_eval,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "vsplit",
        aliases: &["vs"],
        doc: "Open the file in a vertical split.",
        fun: typed::vsplit,
        completer: CommandCompleter::all(completers::filename),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "vsplit-new",
        aliases: &["vnew"],
        doc: "Open a scratch buffer in a vertical split.",
        fun: typed::vsplit_new,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "hsplit",
        aliases: &["hs", "sp"],
        doc: "Open the file in a horizontal split.",
        fun: typed::hsplit,
        completer: CommandCompleter::all(completers::filename),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "hsplit-new",
        aliases: &["hnew"],
        doc: "Open a scratch buffer in a horizontal split.",
        fun: typed::hsplit_new,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "tutor",
        aliases: &[],
        doc: "Open the tutorial.",
        fun: typed::tutor,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "goto",
        aliases: &["g"],
        doc: "Goto line number.",
        fun: typed::goto_line_number,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "set-language",
        aliases: &["lang"],
        doc: "Set the language of current buffer (show current language if no value specified).",
        fun: typed::language,
        completer: CommandCompleter::positional(&[completers::language]),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "set-spelling-language",
        aliases: &["spelling"],
        doc: "Set the spell-checking languages for the current buffer (e.g. `en_US`); a word is flagged only when every language rejects it. Pass `off` to disable, or no value to show the current languages.",
        fun: typed::spelling_language,
        completer: CommandCompleter::all(completers::spelling_language),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "set-option",
        aliases: &["set"],
        doc: "Set a config option at runtime.\nFor example to disable smart case search, use `:set search.smart-case false`.",
        fun: typed::set_option,
        // TODO: Add support for completion of the options value(s), when appropriate.
        completer: CommandCompleter::positional(&[completers::setting]),
        signature: Signature {
            positionals: (2, Some(2)),
            raw_after: Some(1),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "toggle-option",
        aliases: &["toggle"],
        doc: "Toggle a config option at runtime.\nFor example to toggle smart case search, use `:toggle search.smart-case`.",
        fun: typed::toggle_option,
        completer: CommandCompleter::positional(&[completers::setting]),
        signature: Signature {
            positionals: (1, None),
            raw_after: Some(1),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "get-option",
        aliases: &["get"],
        doc: "Get the current value of a config option.",
        fun: typed::get_option,
        completer: CommandCompleter::positional(&[completers::setting]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "sort",
        aliases: &[],
        doc: "Sort ranges in selection.",
        fun: typed::sort,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            flags: &[
                Flag {
                    name: "insensitive",
                    alias: Some('i'),
                    doc: "sort the ranges case-insensitively",
                    ..Flag::DEFAULT
                },
                Flag {
                    name: "reverse",
                    alias: Some('r'),
                    doc: "sort ranges in reverse order",
                    ..Flag::DEFAULT
                },
            ],
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "reflow",
        aliases: &[],
        doc: "Hard-wrap the current selection of lines to a given width.",
        fun: typed::reflow,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "tree-sitter-subtree",
        aliases: &["ts-subtree"],
        doc: "Display the smallest tree-sitter subtree that spans the primary selection, primarily for debugging queries.",
        fun: typed::tree_sitter_subtree,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "config-reload",
        aliases: &[],
        doc: "Refresh user config.",
        fun: typed::refresh_config,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "config-open",
        aliases: &[],
        doc: "Open the user config.toml file.",
        fun: typed::open_config,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "config-open-workspace",
        aliases: &[],
        doc: "Open the workspace config.toml file.",
        fun: typed::open_workspace_config,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "log-open",
        aliases: &[],
        doc: "Open the mitos log file.",
        fun: typed::open_log,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "insert-output",
        aliases: &[],
        doc: "Run shell command, inserting output before each selection.",
        fun: typed::insert_output,
        completer: SHELL_COMPLETER,
        signature: SHELL_SIGNATURE,
    },
    TypableCommand {
        name: "append-output",
        aliases: &[],
        doc: "Run shell command, appending output after each selection.",
        fun: typed::append_output,
        completer: SHELL_COMPLETER,
        signature: SHELL_SIGNATURE,
    },
    TypableCommand {
        name: "pipe",
        aliases: &["|"],
        doc: "Pipe each selection to the shell command.",
        fun: typed::pipe,
        completer: SHELL_COMPLETER,
        signature: SHELL_SIGNATURE,
    },
    TypableCommand {
        name: "pipe-to",
        aliases: &[],
        doc: "Pipe each selection to the shell command, ignoring output.",
        fun: typed::pipe_to,
        completer: SHELL_COMPLETER,
        signature: SHELL_SIGNATURE,
    },
    TypableCommand {
        name: "run-shell-command",
        aliases: &["sh", "!"],
        doc: "Run a shell command",
        fun: typed::run_shell_command,
        completer: SHELL_COMPLETER,
        signature: SHELL_SIGNATURE,
    },
    TypableCommand {
        name: "reset-diff-change",
        aliases: &["diffget", "diffg"],
        doc: "Reset the diff change at the cursor position.",
        fun: typed::reset_diff_change,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "clear-register",
        aliases: &[],
        doc: "Clear given register. If no argument is provided, clear all registers.",
        fun: typed::clear_register,
        completer: CommandCompleter::all(completers::register),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "set-register",
        aliases: &[],
        doc: "Set contents of the given register.",
        fun: typed::set_register,
        completer: CommandCompleter::positional(&[completers::register, completers::none]),
        signature: Signature {
            positionals: (2, Some(2)),
            raw_after: Some(1),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "redraw",
        aliases: &[],
        doc: "Clear and re-render the whole UI",
        fun: typed::redraw,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "move",
        aliases: &["mv"],
        doc: "Move the current buffer and its corresponding file to a different path",
        fun: typed::move_buffer,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "move!",
        aliases: &["mv!"],
        doc: "Move the current buffer and its corresponding file to a different path creating necessary subdirectories",
        fun: typed::force_move_buffer,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "yank-diagnostic",
        aliases: &[],
        doc: "Yank diagnostic(s) under primary cursor to register, or clipboard by default",
        fun: typed::yank_diagnostic,
        completer: CommandCompleter::all(completers::register),
        signature: Signature {
            positionals: (0, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "read",
        aliases: &["r"],
        doc: "Load a file into buffer",
        fun: typed::read,
        completer: CommandCompleter::positional(&[completers::filename]),
        signature: Signature {
            positionals: (1, Some(1)),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "echo",
        aliases: &[],
        doc: "Prints the given arguments to the statusline.",
        fun: typed::echo,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (1, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "noop",
        aliases: &[],
        doc: "Does nothing.",
        fun: typed::noop,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, None),
            ..Signature::DEFAULT
        },
    },
    TypableCommand {
        name: "workspace-trust",
        aliases: &[],
        doc: "Allow language servers and local config for the current workspace.",
        fun: typed::trust_workspace,
        completer: CommandCompleter::none(),
        signature: Signature { positionals: (0, None), ..Signature::DEFAULT },
    },
    TypableCommand {
        name: "workspace-untrust",
        aliases: &[],
        doc: "Revoke the current workspace's trust grant or exclusion.",
        fun: typed::untrust_workspace,
        completer: CommandCompleter::none(),
        signature: Signature { positionals: (0, None), ..Signature::DEFAULT },
    },
    TypableCommand {
        name: "workspace-exclude",
        aliases: &[],
        doc: "Mark the current workspace as never-prompt. Never prompts for trust again.",
        fun: typed::exclude_workspace,
        completer: CommandCompleter::none(),
        signature: Signature { positionals: (0, None), ..Signature::DEFAULT },
    }
];

pub static TYPABLE_COMMAND_MAP: LazyLock<HashMap<&'static str, &'static TypableCommand>> =
    LazyLock::new(|| {
        TYPABLE_COMMAND_LIST
            .iter()
            .flat_map(|cmd| {
                std::iter::once((cmd.name, cmd))
                    .chain(cmd.aliases.iter().map(move |&alias| (alias, cmd)))
            })
            .collect()
    });
