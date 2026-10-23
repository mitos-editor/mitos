#!/usr/bin/env bash
# Bash completion script for Mitos editor

_ms() {
    local cur prev languages
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD - 1]}"

    case "$prev" in
    -g | --grammar)
        mapfile -t COMPREPLY < <(compgen -W 'fetch build' -- "$cur")
        return 0
        ;;
    --health)
        languages=$(ms --health all-languages | tail -n '+2' | awk '{print $1}' | sed 's/\x1b\[[0-9;]*m//g')
        mapfile -t COMPREPLY < <(compgen -W """clipboard languages all-languages all $languages""" -- "$cur")
        return 0
        ;;
    esac

    case "$2" in
    -*)
        mapfile -t COMPREPLY < <(compgen -W "-h --help --strict --tutor -V --version -v -vv -vvv --health -g --grammar --vsplit --hsplit -c --config -t --theme --migrate-config --stdout --log" -- """$2""")
        return 0
        ;;
    *)
        mapfile -t COMPREPLY < <(compgen -fd -- """$2""")
        return 0
        ;;
    esac
} && complete -o filenames -F _ms ms
