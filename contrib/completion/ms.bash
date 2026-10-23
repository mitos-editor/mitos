#!/usr/bin/env bash
# Bash completion script for Mitos editor

_ms() {
    local cur prev languages line
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD - 1]}"

    case "$prev" in
    -g | --grammar)
        while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -W 'fetch build' -- "$cur")
        return 0
        ;;
    --health)
        languages=$(ms --health all-languages | tail -n '+2' | awk '{print $1}' | sed 's/\x1b\[[0-9;]*m//g')
        while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -W """clipboard languages all-languages all $languages""" -- "$cur")
        return 0
        ;;
    esac

    case "$2" in
    -*)
        while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -W "-h --help --strict --tutor -V --version -v -vv -vvv --health -g --grammar --vsplit --hsplit -c --config --migrate-config --stdout --log" -- """$2""")
        return 0
        ;;
    *)
        while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -fd -- """$2""")
        return 0
        ;;
    esac
} && complete -o filenames -F _ms ms
