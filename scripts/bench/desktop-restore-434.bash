# Invoked only as --rcfile by desktop-restore-434.mjs.
HISTFILE=${LYREL_HISTFILE:-/dev/null}
shopt -s histappend

restore434_prompt() {
    local cwd=$PWD
    if [[ $OSTYPE == msys* || $OSTYPE == cygwin* ]]; then
        cwd=/$(pwd -W)
    fi
    printf '\033]7;file://localhost%s\007' "${cwd//%/%25}"
    PS1="[${PWD##*/}] \\$ "
}
PROMPT_COMMAND=restore434_prompt

b434() {
    [[ $# == 1 && $1 =~ ^[a-z0-9]+$ ]] || return 2
    local cwd=$PWD leaf=${PWD##*/}
    if [[ $OSTYPE == msys* || $OSTYPE == cygwin* ]]; then
        cwd=$(pwd -W)
    fi
    printf 'R434:%s:%s\n' "$1" "$leaf"
    printf '%s\t%s\t%s\t%s\n' "$1" "$LYREL_PANE" "$LYREL_WS" "$cwd" >> "$LYREL_RESTORE434_ACK"
    history -a
}
