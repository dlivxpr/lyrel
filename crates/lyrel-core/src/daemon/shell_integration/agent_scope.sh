# Shared Bash/zsh literal-simple-command recognizer. Never eval user input.
# Reject operators/substitutions, never interpret an arbitrary shell expression.
# A final & is accepted; quoted literal arguments/assignments are untouched.
__lyrel_scope_word() {
  local key base
  if [[ -z "$__lyrel_scope_agent" ]]; then
    if [[ "$__lyrel_scope_token" == *=* ]]; then
      key=${__lyrel_scope_token%%=*}
      [[ "$key" == [a-zA-Z_]* && "$key" != *[!a-zA-Z_0-9]* ]] || return 1
      __lyrel_scope_explicit="$__lyrel_scope_explicit $key "
      return 0
    fi
    base=${__lyrel_scope_token##*/}
    case "$__lyrel_scope_token" in */*) [[ "$__lyrel_scope_token" == /* ]] || return 1 ;; esac
    case "$base" in
      omp|kimi|pi|opencode|opencode2|hermes) __lyrel_scope_agent=$base ;;
      *) return 1 ;;
    esac
  fi
}
__lyrel_scope_recognize() {
  local line=$1 char quote= escaped= token_started=
  __lyrel_scope_agent= __lyrel_scope_explicit=' ' __lyrel_scope_token=
  while [[ -n "$line" ]]; do
    char=${line%"${line#?}"}; line=${line#?}
    if [[ -z "$escaped" && "$quote" != "'" ]]; then
      case "$char" in '$'|'`') return 1 ;; esac
      if [[ -z "$quote" ]]; then
        case "$char" in
          '('|')'|'{'|'}'|';'|'|'|'<'|'>'|'#'|$'\n'|$'\r') return 1 ;;
          '&') [[ -n "$__lyrel_scope_agent$__lyrel_scope_token" && "$line" != *[![:space:]]* ]] || return 1; line=; break ;;
        esac
      fi
    fi
    if [[ -n "$escaped" ]]; then
      __lyrel_scope_token=$__lyrel_scope_token$char; escaped=; token_started=1
    elif [[ "$char" == '\' && "$quote" != "'" ]]; then
      escaped=1
    elif [[ -n "$quote" ]]; then
      if [[ "$char" == "$quote" ]]; then quote=; else __lyrel_scope_token=$__lyrel_scope_token$char; fi
    elif [[ "$char" == "'" || "$char" == '"' ]]; then
      quote=$char; token_started=1
    elif [[ "$char" == ' ' || "$char" == $'\t' ]]; then
      if [[ -n "$token_started" ]]; then
        __lyrel_scope_word || return 1
        __lyrel_scope_token= token_started=
      fi
    else
      __lyrel_scope_token=$__lyrel_scope_token$char; token_started=1
    fi
  done
  [[ -z "$quote$escaped" ]] || return 1
  if [[ -n "$token_started" ]]; then __lyrel_scope_word || return 1; fi
  [[ -n "$__lyrel_scope_agent" ]]
}
__lyrel_scope_restore() {
  if [[ -n "${__lyrel_scope_undo+x}" ]]; then
    # This contains only builtin-quoted values of the six fixed terminal keys.
    builtin eval "$__lyrel_scope_undo"
    unset __lyrel_scope_undo
  fi
}
__lyrel_scope_apply() {
  [[ -z "${__lyrel_scope_undo+x}" ]] || return 0
  __lyrel_scope_recognize "$1" || return 0
  local key value present saved declaration quoted
  __lyrel_scope_undo=
  for key in TERM_PROGRAM TERM_PROGRAM_VERSION LC_TERMINAL LC_TERMINAL_VERSION PI_NO_DECCARA KITTY_WINDOW_ID; do
    [[ "$__lyrel_scope_explicit" != *" $key "* ]] || continue
    case "$key" in
      PI_NO_DECCARA) [[ "$__lyrel_scope_agent" == omp ]] || continue; value=1 ;;
      KITTY_WINDOW_ID)
        [[ "$__lyrel_scope_agent" == hermes ]] || continue
        builtin eval 'present=${'"$key"'+x}'
        [[ -z "$present" ]] || continue
        value=lyrel ;;
      TERM_PROGRAM) value=kitty ;;
      TERM_PROGRAM_VERSION) value= ;;
      LC_TERMINAL) value=lyrel ;;
      LC_TERMINAL_VERSION) value=${LYREL-$__lyrel_scope_version} ;;
    esac
    builtin eval 'present=${'"$key"'+x}; saved=${'"$key"'-}'
    if [[ -n "$present" ]]; then
      builtin printf -v quoted '%q' "$saved"
      __lyrel_scope_undo="$__lyrel_scope_undo export $key=$quoted;"
      declaration=$(typeset -p "$key" 2>/dev/null)
      case "$declaration" in
        *' -x '*|export\ *) ;;
        *)
          if [[ -n "${ZSH_VERSION-}" ]]; then
            __lyrel_scope_undo="$__lyrel_scope_undo typeset -g +x $key;"
          else
            __lyrel_scope_undo="$__lyrel_scope_undo export -n $key;"
          fi ;;
      esac
    else
      __lyrel_scope_undo="$__lyrel_scope_undo unset $key;"
    fi
    export "$key=$value"
  done
}
