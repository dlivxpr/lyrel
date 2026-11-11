  function __lyrel_scope_restore
    if not set -q __lyrel_scope_keys
      return
    end
    for key in $__lyrel_scope_keys
      set -l saved __lyrel_scope_saved_$key
      set -l existed __lyrel_scope_set_$key
      set -l exported __lyrel_scope_export_$key
      if test "$$existed" = 1
        set -g $key $$saved
        if test "$$exported" = 1
          set -gx $key $$saved
        else
          set -gu $key $$saved
        end
      else
        set -eg $key
      end
      set -eg $saved $existed $exported
    end
    set -eg __lyrel_scope_keys
  end

  function __lyrel_scope_apply
    set -q __lyrel_scope_keys; and return
    set -l line (string replace -r '&\s*$' '' -- "$argv[1]")
    # Strip literal quoted spans only for operator validation, not tokenizing.
    set -l syntax (string replace -ra "'[^']*'|\"[^\"\x24`]*\"" '' -- "$line" | string replace -ra '\\\\.' '' )
    string match -qr '[;&|<>$`(){}\n\r#]' -- "$syntax"; and return
    set -l tokens
    printf '%s\n' "$line" | read --tokenize --array tokens
    or return
    set -l explicit
    set -l agent
    for token in $tokens
      if test -z "$agent"
        if string match -qr '^[A-Za-z_][A-Za-z_0-9]*=' -- $token
          set -a explicit (string split -m 1 '=' -- $token)[1]
          continue
        end
        if string match -q '*/*' -- $token; and not string match -q '/*' -- $token
          return
        end
        set agent (string replace -r '^.*/' '' -- $token)
        contains -- $agent omp kimi pi opencode opencode2 hermes; or return
      end
    end
    test -n "$agent"; or return
    set -g __lyrel_scope_keys
    for key in TERM_PROGRAM TERM_PROGRAM_VERSION LC_TERMINAL LC_TERMINAL_VERSION PI_NO_DECCARA KITTY_WINDOW_ID
      contains -- $key $explicit; and continue
      set -l value
      switch $key
        case TERM_PROGRAM
          set value kitty
        case TERM_PROGRAM_VERSION
          set value ''
        case LC_TERMINAL
          set value lyrel
        case LC_TERMINAL_VERSION
          if set -q LYREL
            set value "$LYREL"
          else
            set value "$__lyrel_scope_version"
          end
        case PI_NO_DECCARA
          test "$agent" = omp; or continue
          set value 1
        case KITTY_WINDOW_ID
          test "$agent" = hermes; or continue
          set -q $key; and continue
          set value lyrel
      end
      set -a __lyrel_scope_keys $key
      set -g __lyrel_scope_saved_$key $$key
      if set -q $key
        set -g __lyrel_scope_set_$key 1
      else
        set -g __lyrel_scope_set_$key 0
      end
      if set -qx $key
        set -g __lyrel_scope_export_$key 1
      else
        set -g __lyrel_scope_export_$key 0
      end
      set -gx $key $value
    end
  end
