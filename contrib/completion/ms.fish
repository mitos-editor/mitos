#!/usr/bin/env fish
# Fish completion script for Mitos editor

complete -c ms -s h -l help -d "Prints help information"
complete -c ms -l strict -d "Bail on error for commands that can fail"
complete -c ms -l tutor -d "Loads the tutorial"
complete -c ms -l health -xa "(__ms_langs_ops)" -d "Checks for errors"
complete -c ms -l health -xka all -d "Prints all diagnostic informations"
complete -c ms -l health -xka all-languages -d "Lists all languages"
complete -c ms -l health -xka languages -d "Lists user configured languages"
complete -c ms -l health -xka clipboard -d "Prints system clipboard provider"
complete -c ms -s g -l grammar -x -a "fetch build" -d "Fetch or build tree-sitter grammars"
complete -c ms -s v -o vv -o vvv -d "Increases logging verbosity"
complete -c ms -s V -l version -d "Prints version information"
complete -c ms -l vsplit -d "Splits all given files vertically"
complete -c ms -l hsplit -d "Splits all given files horizontally"
complete -c ms -s c -l config -r -d "Specifies a file to use for config"
complete -c ms -s t -l theme -r -d "Overrides the configured theme with a theme file"
complete -c ms -l migrate-config -r -d "Export migrated configuration (requires --stdout)"
complete -c ms -l stdout -d "Print migrated configuration without changing the file"
complete -c ms -l log -r -d "Specifies a file to use for logging"
complete -c ms -s w -l working-dir -d "Specify initial working directory" -xa "(__fish_complete_directories)"

function __ms_langs_ops
    ms --health all-languages | tail -n '+2' | string replace -fr '^(\S+) .*' '$1'
end
