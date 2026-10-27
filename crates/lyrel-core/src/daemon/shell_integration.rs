use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ZSH_INTEGRATION: &str = r#"
# --- lyrel shell integration (zsh) ---
if [[ -o interactive ]] && [[ -z "$LYREL_SHELL_INTEGRATION" ]]; then
  export LYREL_SHELL_INTEGRATION=1

  # Per-pane history, when the app asked for it. This runs after the user's
  # .zshrc, which is the only reason it can work at all: $HISTFILE is theirs to
  # set, wherever they like, and nothing outside this shell knew where it
  # points until now. Seed once from there so the pane does not start blank,
  # write down how much was seeded — everything past that mark is what this
  # pane adds, and is what goes home when it closes — and only then repoint.
  # zsh loads the history file after .zshrc, so the switch lands before the
  # first line is read.
  if [[ -n "$LYREL_HISTFILE" ]]; then
    if [[ ! -e "$LYREL_HISTFILE" ]]; then
      # The origin is recorded even when there is nothing to copy from it yet:
      # a first-ever shell has no history file, and it is exactly that user
      # whose commands would otherwise have nowhere to go when the pane closes.
      if [[ -n "$HISTFILE" ]]; then
        builtin printf '%s\n' "$HISTFILE" > "$LYREL_HISTFILE.origin" 2>/dev/null
        if [[ -r "$HISTFILE" ]]; then
          command tail -n 5000 "$HISTFILE" > "$LYREL_HISTFILE" 2>/dev/null
        fi
      fi
      [[ -e "$LYREL_HISTFILE" ]] || : > "$LYREL_HISTFILE" 2>/dev/null
      command wc -c < "$LYREL_HISTFILE" 2>/dev/null | command tr -d ' \n' \
        > "$LYREL_HISTFILE.seed" 2>/dev/null
    fi
    HISTFILE="$LYREL_HISTFILE"
    # The exit rewrite keeps at most SAVEHIST entries, from a memory capped at
    # HISTSIZE. Smaller than the seed, that rewrite leaves the pane's file
    # shorter than its own seed mark — which the merge on close reads as "the
    # file was replaced under us" and rightly refuses, losing this pane's
    # commands. The caps only govern the pane's private file now, so raising
    # them costs nothing. Left at zero: a user who saves no history has asked
    # for exactly that.
    if (( SAVEHIST > 0 )); then
      (( SAVEHIST < 100000 )) && SAVEHIST=100000
      (( HISTSIZE < SAVEHIST )) && HISTSIZE=$SAVEHIST
    fi
  fi

  __lyrel_osc() { builtin printf '\e]%s\a' "$1"; }

  # Vi mode links the `main` keymap to `viins` (`bindkey -A viins main`);
  # emacs mode links it to `emacs`. The link survives plugins like
  # zsh-vi-mode that rebind `^[` to their own widgets, so it beats sniffing
  # the Esc widget name.
  __lyrel_report_edit_mode() {
    if [[ "$(builtin bindkey -lL main)" == *viins* ]]; then
      __lyrel_osc "133;V;1"
    else
      __lyrel_osc "133;V;0"
    fi
  }

  # OSC 7: report the working directory so the app tracks it precisely (used for
  # opening new tabs / splits in the same place). The daemon percent-DECODES the
  # payload (OSC 7 carries a file: URI), so a literal `%` in the path must be
  # escaped as %25 or a dir like `/tmp/a%20b` would decode to `/tmp/a b`.
  __lyrel_report_cwd() { builtin printf '\e]7;file://%s%s\a' "${HOST:-localhost}" "${PWD//\%/%25}"; }

  # D (command finished + its exit code) gets its own hook, *prepended* to
  # precmd_functions rather than bundled into __lyrel_precmd below: the app only
  # switches back to prompt-editing when D arrives, so every hook that runs
  # before it is a window where keystrokes go raw to the PTY, get kernel-echoed
  # into the grid, and bait zsh's PROMPT_SP into leaving a stray `char + %` line.
  # The user's precmd chain (git-status prompts, conda, …) can take hundreds of
  # ms — D must not wait for it.
  __lyrel_precmd_d() {
    local ret=$?
    if [[ -n "$__lyrel_cmd_active" ]]; then
      __lyrel_osc "133;D;$ret;aid=lyrel"
      unset __lyrel_cmd_active
    fi
  }

  # The rest of the prompt bookkeeping runs right before the prompt is drawn,
  # *after* the user's hooks: report cwd, then open a fresh prompt (A).
  __lyrel_precmd() {
    __lyrel_report_cwd
    __lyrel_report_edit_mode
    __lyrel_osc "133;A"
    # Prompt-end marker (B): emitted at the very end of the prompt — exactly where
    # input begins — by living in PS1 (wrapped in %{...%} so zsh excludes it from
    # prompt width). We (re)append it here in precmd rather than once at load,
    # because prompt frameworks (powerlevel10k / starship / oh-my-zsh) rebuild
    # PS1 in their own precmd and would otherwise drop it. This precmd runs last
    # (added after the user's), and the sentinel check keeps a static PS1 from
    # accumulating duplicate markers.
    [[ "$PS1" != *$'\e]133;B\a'* ]] && PS1="$PS1"$'%{\e]133;B\a%}'
  }

  # preexec runs after the user hits Enter, before the command runs: mark the
  # start of command output (C). We track an "active" flag so the very first
  # prompt (no command yet) doesn't emit a bogus D. The C mark carries the
  # submitted line ($1), truncated (detection only reads the front) and with
  # the bytes that would break OSC framing or the daemon's percent-decode
  # escaped (% ESC BEL CR NL) — the coding-agent detection input on Windows,
  # where ConPTY has no process table to poll (see core::cli_agent).
  __lyrel_preexec() {
    __lyrel_cmd_active=1
    local cmd=$1
    cmd=${cmd[1,512]}
    cmd=${cmd//\%/%25}
    cmd=${cmd//$'\e'/%1B}
    cmd=${cmd//$'\a'/%07}
    cmd=${cmd//$'\r'/%0D}
    cmd=${cmd//$'\n'/%0A}
    __lyrel_osc "133;C;$cmd;aid=lyrel"
  }

  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __lyrel_precmd
  add-zsh-hook preexec __lyrel_preexec
  # add-zsh-hook can only append, and the user's hooks are all registered by now
  # (their .zshrc ran before this file) — prepend the D emitter by hand so it's
  # the first thing to run when a command exits. Users who define a classic
  # `precmd()` function still get ahead of us (zsh calls it before the array);
  # that's out of reach without wrapping their function.
  precmd_functions=(__lyrel_precmd_d $precmd_functions)

  # Startup kept ZDOTDIR aimed at our throwaway redirector dir so zsh read every
  # one of our startup files. Now they've all run, point it back at the user's
  # real config dir for the live session: tools that resolve state via
  # ${ZDOTDIR:-$HOME} *at runtime* (compinit's .zcompdump, lazily-compiled plugin
  # caches) — and a nested plain `zsh` — must land in the user's dir, not our
  # empty temp one. One-shot: fires on the first precmd, then removes itself.
  __lyrel_restore_zdotdir() {
    ZDOTDIR=${LYREL_USER_ZDOTDIR:-$HOME}
    add-zsh-hook -d precmd __lyrel_restore_zdotdir
    unfunction __lyrel_restore_zdotdir
  }
  add-zsh-hook precmd __lyrel_restore_zdotdir
fi
# --- end lyrel shell integration ---
"#;

const FISH_INTEGRATION: &str = r#"
# --- lyrel shell integration (fish) ---
# Guard on *emptiness* (`test -z`), not definedness (`set -q`): `setup()` resets the
# sentinel to an empty-but-exported "" at each spawn boundary, and fish reports an
# empty exported var as *set*, so `not set -q` would skip the install on every fish
# launch (OSC 133 never arms). `-z` matches the zsh/bash guards and the empty reset —
# it installs once for a fresh top-level shell while an inherited `1` still blocks it.
if status is-interactive; and test -z "$LYREL_SHELL_INTEGRATION"
  set -gx LYREL_SHELL_INTEGRATION 1

  function __lyrel_osc
    printf '\e]%s\a' $argv[1]
  end

  function __lyrel_report_edit_mode
    switch $fish_key_bindings
      case '*vi*'
        __lyrel_osc "133;V;1"
      case '*'
        __lyrel_osc "133;V;0"
    end
  end

  # The daemon percent-decodes the OSC 7 payload; escape literal `%` as %25 so
  # a path like /tmp/a%20b round-trips instead of decoding to /tmp/a b.
  function __lyrel_report_cwd
    printf '\e]7;file://%s%s\a' (hostname) (string replace --all '%' '%25' -- $PWD)
  end

  # The C mark carries the submitted line, truncated and with the bytes that
  # would break OSC framing or the daemon's percent-decode escaped (% ESC BEL
  # CR NL) — the Windows agent-detection input (see core::cli_agent). fish
  # command substitution splits output on newlines, so a multi-line command
  # arrives as a list; the final `string join` re-joins it with the escaped
  # newline. `%` must be escaped first (the other escapes introduce `%`).
  function __lyrel_preexec --on-event fish_preexec
    set -g __lyrel_cmd_active 1
    set -l cmd (string sub -l 512 -- $argv[1] | string replace -a '%' '%25' | string replace -a \e '%1B' | string replace -a \a '%07' | string replace -a \r '%0D' | string join '%0A')
    __lyrel_osc "133;C;$cmd;aid=lyrel"
  end

  # Runs on the fish_prompt *event*, which fires before fish calls the
  # fish_prompt *function* to render the prompt text — i.e. exactly where A
  # (prompt start) belongs.
  function __lyrel_precmd --on-event fish_prompt
    set -l ret $status
    if set -q __lyrel_cmd_active
      __lyrel_osc "133;D;$ret;aid=lyrel"
      set -e __lyrel_cmd_active
    end
    __lyrel_report_cwd
    __lyrel_report_edit_mode
    __lyrel_osc "133;A"
  end

  functions -c fish_prompt __lyrel_original_fish_prompt
  function fish_prompt
    __lyrel_original_fish_prompt
    __lyrel_osc "133;B"
  end
end
# --- end lyrel shell integration ---
"#;

const BASH_INTEGRATION: &str = r#"
# --- lyrel shell integration (bash) ---
if [[ $- == *i* ]] && [[ -z "$LYREL_SHELL_INTEGRATION" ]]; then
  export LYREL_SHELL_INTEGRATION=1

  # Per-pane history — see the zsh block for why this can only be done here,
  # after the user's rc has decided where their history lives. bash loads the
  # file after the startup files too, so repointing here still precedes the
  # load.
  if [[ -n "$LYREL_HISTFILE" ]]; then
    if [[ ! -e "$LYREL_HISTFILE" ]]; then
      # The origin is recorded even when there is nothing to copy from it yet:
      # a first-ever shell has no history file, and it is exactly that user
      # whose commands would otherwise have nowhere to go when the pane closes.
      if [[ -n "$HISTFILE" ]]; then
        builtin printf '%s\n' "$HISTFILE" > "$LYREL_HISTFILE.origin" 2>/dev/null
        if [[ -r "$HISTFILE" ]]; then
          command tail -n 5000 "$HISTFILE" > "$LYREL_HISTFILE" 2>/dev/null
        fi
      fi
      [[ -e "$LYREL_HISTFILE" ]] || : > "$LYREL_HISTFILE" 2>/dev/null
      command wc -c < "$LYREL_HISTFILE" 2>/dev/null | command tr -d ' \n' \
        > "$LYREL_HISTFILE.seed" 2>/dev/null
    fi
    HISTFILE="$LYREL_HISTFILE"
    # On exit bash rewrites the file from a memory capped at HISTSIZE, then
    # truncates it to HISTFILESIZE lines — both default to 500, which is
    # smaller than the seed. The result would be a file shorter than its own
    # seed mark, which the merge on close reads as "replaced under us" and
    # rightly refuses, losing this pane's commands. Both caps only govern the
    # pane's private file now, so raising them costs nothing. Negative means
    # unlimited and is already enough.
    if (( ${HISTSIZE:-500} >= 0 && ${HISTSIZE:-500} < 100000 )) 2>/dev/null; then
      HISTSIZE=100000
    fi
    if (( ${HISTFILESIZE:-500} >= 0 && ${HISTFILESIZE:-500} < 100000 )) 2>/dev/null; then
      HISTFILESIZE=100000
    fi
  fi

  __lyrel_osc() { builtin printf '\e]%s\a' "$1"; }
  # `bind -v` reports readline's actual editing mode; `[[ -o vi ]]` misses
  # vi mode configured only in ~/.inputrc (`set editing-mode vi` flips
  # readline without setting the shell option).
  __lyrel_report_edit_mode() {
    if [[ "$(builtin bind -v 2>/dev/null)" == *"set editing-mode vi"* ]]; then
      __lyrel_osc "133;V;1"
    else
      __lyrel_osc "133;V;0"
    fi
  }
  # Escape literal `%` as %25 — the daemon percent-decodes the OSC 7 payload.
  #
  # Under Git Bash (msys) `$PWD` is an msys path — `/c/Users/x`, and `/tmp` for
  # mounts with no drive at all. The daemon runs Windows-side, where `/c/Users/x`
  # is not absolute but *drive-relative*, so it resolves to a bogus `C:\c\Users\x`
  # (see `pane::strip_uri_drive_slash`, which only un-prefixes `/C:/…`). `pwd -W`
  # is msys's own translation to the real Windows path, and it resolves mounts
  # that have a real backing directory (`/tmp` -> `C:/Users/x/AppData/Local/Temp`).
  # It has no leading slash, so add one to make it the absolute-path shape a
  # file: URI expects.
  #
  # For msys-only virtual mounts (`/proc`, `/dev`) there *is* no Windows path
  # and `pwd -W` is the identity, so require a drive letter and stay silent
  # otherwise — a `/proc` payload would land as drive-relative `C:\proc` and
  # fail the next spawn, whereas reporting nothing leaves the daemon holding
  # the last usable cwd. Testing the shape beats testing for a leading slash,
  # which cannot tell a translated path from an untranslated one.
  #
  # The branch is resolved once at install time; the `$(…)` inside still forks
  # per prompt (~6.8 ms under msys, vs ~0.3 ms for the plain `$PWD` path), which
  # is the price of a correct path and only paid by Git Bash panes.
  if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]]; then
    __lyrel_report_cwd() {
      local d
      d="$(builtin pwd -W 2>/dev/null)" || return 0
      [[ "$d" == ?:* ]] || return 0
      builtin printf '\e]7;file://%s/%s\a' "${HOSTNAME:-localhost}" "${d//\%/%25}"
    }
  else
    __lyrel_report_cwd() { builtin printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "${PWD//\%/%25}"; }
  fi

  # D must precede the user's prompt hooks: it returns input to the editor
  # before slow prompt frameworks start running.
  __lyrel_precmd_d() {
    local ret=${1:-$?}
    if [[ -n "${__lyrel_cmd_active+x}" ]]; then
      __lyrel_osc "133;D;$ret;aid=lyrel"
    fi
    return $ret
  }

  __lyrel_precmd() {
    local ret=$?
    unset __lyrel_cmd_active
    # The SSH bootstrap supplies this one-shot cleanup after loading us.
    if [[ -n "${__lyrel_rm_dir+x}" ]]; then __lyrel_rm_rcdir; fi
    __lyrel_report_cwd
    __lyrel_report_edit_mode
    __lyrel_osc "133;A"
    # Prompt-end marker (B), wrapped in \[...\] so readline excludes it from the
    # prompt's on-screen width. Re-appended every precmd (like the zsh path)
    # since prompt frameworks that rebuild PS1 in their own precmd would
    # otherwise drop it; the case-check keeps a static PS1 from accumulating
    # duplicates.
    case "$PS1" in
      *'\[\033]133;B\a\]'*) ;;
      *) PS1="$PS1"'\[\033]133;B\a\]' ;;
    esac
    __lyrel_prompt_state=ready
    return $ret
  }

  # The C mark carries the submitted line, truncated and escaped the same way
  # as the zsh path — the Windows agent-detection input (git-bash).
  __lyrel_preexec() {
    __lyrel_cmd_active=1
    local cmd=${1:0:512}
    cmd=${cmd//\%/%25}
    cmd=${cmd//$'\e'/%1B}
    cmd=${cmd//$'\a'/%07}
    cmd=${cmd//$'\r'/%0D}
    cmd=${cmd//$'\n'/%0A}
    __lyrel_osc "133;C;$cmd;aid=lyrel"
  }

  # 历史：这里曾内嵌 https://github.com/rcaloras/bash-preexec 的 MIT 实现；
  # 该第三方库副本现已移除，来源与许可记录保留在此及 Git 历史中。
  # 用户自行安装该库时，只使用其公开的函数数组，不安装或下载任何库。
  if [[ -n "${bash_preexec_imported:-}" ]]; then
    precmd_functions=(__lyrel_precmd_d "${precmd_functions[@]}")
    precmd_functions+=(__lyrel_precmd)
    preexec_functions+=(__lyrel_preexec)
  else
    __lyrel_submitted_command() {
      local entry
      entry=$(HISTTIMEFORMAT= builtin history 1)
      # `history` prefixes only its first line with an event number. Remove
      # that prefix without an external parser, retaining embedded newlines.
      if [[ "$entry" =~ ^[[:space:]]*[0-9]+[[:space:]*][[:space:]](.*)$ ]]; then
        REPLY=${BASH_REMATCH[1]}
      else
        REPLY=$1
      fi
    }

    if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
      __lyrel_command_start() {
        local REPLY
        __lyrel_submitted_command "$BASH_COMMAND"
        __lyrel_preexec "$REPLY"
      }
      # PS0 is Bash's public once-per-submitted-command event (4.4+). The
      # empty assignment is expanded in the parent shell; only the emission
      # runs in a command substitution. Preserve the user's PS0 verbatim.
      # https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html
      PS0='${__lyrel_cmd_active=}$(__lyrel_command_start)'"${PS0-}"
    else
      # macOS still ships Bash 3.2, without PS0. Use DEBUG only there, leaving
      # history options and functrace/extdebug untouched. The original action
      # stays inline, not wrapped in a function with different positional args.
      __lyrel_debug_action=$(builtin trap -p DEBUG)
      if [[ -n "$__lyrel_debug_action" ]]; then
        __lyrel_debug_action=${__lyrel_debug_action#trap -- }
        __lyrel_debug_action=${__lyrel_debug_action% DEBUG}
        builtin eval "__lyrel_debug_action=$__lyrel_debug_action"
      else
        __lyrel_debug_action=:
      fi
      __lyrel_command_start() {
        if [[ "$2" == 'for __lyrel_prompt_state in prompt'* ]]; then
          __lyrel_prompt_state=prompt
          return "$1"
        fi
        local frame
        for frame in "${FUNCNAME[@]:1}"; do
          [[ "$frame" == __lyrel_* ]] && return "$1"
        done
        [[ "${__lyrel_prompt_state:-}" == ready ]] || return "$1"
        [[ -z "${COMP_LINE:-}" && -z "${READLINE_LINE+x}" ]] || return "$1"
        __lyrel_prompt_state=command
        local REPLY
        __lyrel_submitted_command "$2"
        __lyrel_preexec "$REPLY"
        return "$1"
      }
      # Run the existing action first, with no command ahead of it to replace
      # $? / PIPESTATUS / $_. Our tail receives and returns its result, so
      # extdebug retains the user's command-skip behavior as well.
      builtin trap "$__lyrel_debug_action"$'\n__lyrel_command_start "$?" "$BASH_COMMAND"' DEBUG
      unset __lyrel_debug_action
    fi

    # Expand D in a no-op redirection on the first user prompt command group.
    # Unlike a prepended simple command, this does not replace its incoming
    # $? / PIPESTATUS / $_. The child emits on the original stderr (as Bash
    # does for prompts); it must not clear the parent's active flag.
    # The loop assignment disarms the legacy DEBUG hook even on empty input.
    __lyrel_prompt_open=$'for __lyrel_prompt_state in prompt; do\n{\n'
    __lyrel_prompt_close=$'\n} 2>&2$(__lyrel_precmd_d "$?" >&2)\ndone'
    if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
        [[ "$(builtin declare -p PROMPT_COMMAND 2>/dev/null)" == 'declare -a '* ]]; then
      # Keep all array elements in order, including sparse arrays and strings
      # containing semicolons/newlines. Bash 5.1 executes each element itself.
      __lyrel_prompt_indices=("${!PROMPT_COMMAND[@]}")
      __lyrel_prompt_first=${__lyrel_prompt_indices[0]:-0}
      PROMPT_COMMAND[$__lyrel_prompt_first]="$__lyrel_prompt_open""${PROMPT_COMMAND[$__lyrel_prompt_first]:-:}""$__lyrel_prompt_close"
      PROMPT_COMMAND+=(__lyrel_precmd)
      unset __lyrel_prompt_indices __lyrel_prompt_first
    else
      PROMPT_COMMAND="$__lyrel_prompt_open""${PROMPT_COMMAND:-:}""$__lyrel_prompt_close"$'\n__lyrel_precmd'
    fi
    unset __lyrel_prompt_open __lyrel_prompt_close
  fi
fi
# --- end lyrel shell integration ---
"#;

const POWERSHELL_INTEGRATION: &str = r#"
# --- lyrel shell integration (PowerShell) ---
if (-not $env:LYREL_SHELL_INTEGRATION) {
  $env:LYREL_SHELL_INTEGRATION = '1'

  $global:__LyrelEsc = [char]0x1b
  $global:__LyrelBel = [char]0x07
  # Whatever prompt the user's profile settled on; we call through to it.
  $global:__LyrelOrigPrompt = $function:prompt
  # Gates the D marker so the first prompt (no command yet) emits no bogus exit.
  $global:__LyrelCmdActive = $false

  # Who and where, for the OSC 7 host and the OSC 0 title. Read through .NET
  # rather than the USERNAME / COMPUTERNAME / USERPROFILE environment variables:
  # those spellings exist only on Windows, and on macOS and Linux all three come
  # back empty, which left every pwsh pane there titled `@:` followed by a full
  # un-abbreviated path (issue #583). $HOME is PowerShell's own automatic
  # variable and is right on every platform. Resolved once — none of the three
  # changes within a session.
  $global:__LyrelUser = [Environment]::UserName
  $global:__LyrelHost = [Environment]::MachineName
  $global:__LyrelHome = if ($HOME) { $HOME.Replace('\', '/').TrimEnd('/') } else { '' }

  function global:prompt {
    # $? first: an assignment sets $? to true, so read it before anything else.
    $ok = $?
    $lastExit = $LASTEXITCODE

    if ($global:__LyrelCmdActive) {
      $global:__LyrelCmdActive = $false
      # $? is the reliable success signal; $LASTEXITCODE can be stale, so only
      # trust it when $? already says the command failed.
      $code = if ($ok) { 0 } elseif ($lastExit) { $lastExit } else { 1 }
      Write-Host -NoNewline "$($global:__LyrelEsc)]133;D;$code;aid=lyrel$($global:__LyrelBel)"
    }

    # cwd + title, for real filesystem locations only.
    if ($PWD.Provider.Name -eq 'FileSystem') {
      $fsPath = $PWD.ProviderPath

      # OSC 7 cwd. Escape a literal % as %25 (the daemon percent-decodes the
      # payload) and use forward slashes. Force one leading slash so a Windows
      # drive path (`C:/…`) becomes `/C:/…` — the absolute-path shape the URI
      # expects — while a POSIX path keeps its single slash instead of doubling it.
      $p = $fsPath.Replace('%', '%25').Replace('\', '/')
      if (-not $p.StartsWith('/')) { $p = '/' + $p }
      Write-Host -NoNewline "$($global:__LyrelEsc)]7;file://$($global:__LyrelHost)$p$($global:__LyrelBel)"

      # OSC 0 window/tab title "user@host:dir". Neither a PowerShell profile nor
      # pwsh itself sets a useful title — on macOS pwsh emits an *empty* OSC 0 —
      # so without this every lyrel tab running PowerShell stays generic, on every
      # platform. Forward slashes (so lyrel's tab-label parser can take the last
      # path segment) and home shown as `~`. Re-emitted each prompt so it tracks
      # cwd; a full-screen app's own title still overrides it while it runs.
      #
      # The home match needs the separator, not just the prefix: with a home of
      # `/Users/ann`, a bare StartsWith also swallows `/Users/annex`, retitling it
      # `~ex`.
      $titlePath = $fsPath.Replace('\', '/')
      if ($global:__LyrelHome -and ($titlePath -eq $global:__LyrelHome -or
          $titlePath.StartsWith($global:__LyrelHome + '/'))) {
        $titlePath = '~' + $titlePath.Substring($global:__LyrelHome.Length)
      }
      Write-Host -NoNewline "$($global:__LyrelEsc)]0;$($global:__LyrelUser)@$($global:__LyrelHost):$titlePath$($global:__LyrelBel)"
    }

    # Restore the captured status so the user's own prompt sees the real result,
    # then re-restore $LASTEXITCODE afterwards in case the prompt clobbered it.
    $global:LASTEXITCODE = $lastExit
    if (-not $ok) { Write-Error '' -ErrorAction Ignore }
    $base = & $global:__LyrelOrigPrompt
    if ($base -is [array]) { $base = $base -join [char]0x0a }
    $global:LASTEXITCODE = $lastExit

    "$($global:__LyrelEsc)]133;A$($global:__LyrelBel)$base$($global:__LyrelEsc)]133;B$($global:__LyrelBel)"
  }

  # C (command output begins): wrap PSReadLine's line reader. Force-load the
  # module first so the function exists even if the host hasn't imported it yet;
  # all best-effort (an empty Enter submits no command, so it arms nothing).
  Import-Module PSReadLine -ErrorAction SilentlyContinue
  if ((Test-Path Function:\PSConsoleHostReadLine) -and -not $global:__LyrelReadLineWrapped) {
    $global:__LyrelReadLineWrapped = $true
    $global:__LyrelOrigReadLine = $function:global:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
      $line = & $global:__LyrelOrigReadLine
      if (-not [string]::IsNullOrWhiteSpace($line)) {
        $global:__LyrelCmdActive = $true
        # Carry the submitted line on the C mark (lyrel extension): the daemon
        # detects coding agents from it on Windows, where ConPTY exposes no
        # foreground process group to read an argv from. Truncated (detection
        # only reads the front) and percent-encoded so the payload can't carry
        # a raw `;`, ESC or BEL into the OSC framing. The cut can split a
        # surrogate pair, and a lone high surrogate makes EscapeDataString
        # throw on .NET Framework (PS 5.1) — inside this wrapper that would
        # swallow the submitted line, so drop it and keep the whole mark
        # best-effort: a plain `C` still flips the prompt state.
        $cmd = if ($line.Length -gt 512) { $line.Substring(0, 512) } else { $line }
        if ([char]::IsHighSurrogate($cmd[$cmd.Length - 1])) {
          $cmd = $cmd.Substring(0, $cmd.Length - 1)
        }
        try {
          $cmd = [Uri]::EscapeDataString($cmd)
          Write-Host -NoNewline "$($global:__LyrelEsc)]133;C;$cmd;aid=lyrel$($global:__LyrelBel)"
        } catch {
          Write-Host -NoNewline "$($global:__LyrelEsc)]133;C;;aid=lyrel$($global:__LyrelBel)"
        }
      }
      $line
    }
  }
}
# --- end lyrel shell integration ---
"#;

const NUSHELL_INTEGRATION: &str = r#"
# --- lyrel shell integration (nushell) ---
# Guard on emptiness (`== ""`), not mere definedness, exactly like the other
# shells: setup() resets the sentinel to an empty-but-exported value at each
# spawn boundary, and a nested `nu` inherits the set `1` without re-installing.
if (($env.LYREL_SHELL_INTEGRATION? | default "") == "") {
  $env.LYREL_SHELL_INTEGRATION = "1"

  # lyrel restores the user's own config.nu at this placeholder — `source` is
  # evaluated at parse time, so the wrapper can only name a file that provably
  # exists; the Rust side resolves the path (the same resolution
  # `$nu.default-config-dir` performs) and substitutes a literal, or a no-op
  # line when there is no config.nu. The order is load-bearing: a config.nu
  # conventionally ends with a wholesale `$env.config = {...}` that would
  # clobber every hook added below, so the hooks must come after it.
  __LYREL_SOURCE_USER_CONFIG__

  # Hooks live in `$env.config.hooks` as lists. Ensure the skeleton exists
  # before appending to it — a minimal config, or none at all, leaves it
  # missing, and a bare assignment would error instead of creating it.
  if (($env.config.hooks? | default {}) | is-empty) {
    $env.config = ($env.config | upsert hooks { pre_prompt: [], pre_execution: [], env_change: {} })
  }

  $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | default [] | append {||
    # OSC 133 prompt-start (A) and the previous command's exit (D). D is gated
    # on a flag the pre_execution hook arms, so the very first prompt emits no
    # bogus exit status — the same rule every other integration follows.
    let __lyrel_osc = {|__lyrel_payload| print -n $"(ansi osc)($__lyrel_payload)(char bel)" }
    if ($env.__lyrel_cmd_active? | default false) {
      hide-env __lyrel_cmd_active
      do $__lyrel_osc $"133;D;($env.LAST_EXIT_CODE? | default 0);aid=lyrel"
    }
    # OSC 7 cwd, so the daemon tracks the pane's directory across `cd`. The
    # payload is percent-decoded on the daemon side, so a literal `%` must be
    # escaped as %25, and a Windows drive path needs the leading slash that
    # makes `C:/…` an absolute URI path (`/C:/…`). Backslashes are separators
    # only on Windows — a Unix path may legally contain one, so that
    # translation is gated on the platform the way nu's own OSC 7 gates it.
    let __lyrel_path = if (($nu.os-info.name? | default '') == 'windows') {
      ($env.PWD | str replace -a '\' '/' | str replace -a '%' '%25')
    } else {
      ($env.PWD | str replace -a '%' '%25')
    }
    let __lyrel_path = if ($__lyrel_path | str starts-with '/') { $__lyrel_path } else { '/' + $__lyrel_path }
    do $__lyrel_osc $"7;file://($env.COMPUTERNAME? | default 'localhost')($__lyrel_path)"
    do $__lyrel_osc "133;A"
  })

  # OSC 133 command-output-begins (C) plus the flag that gates the D report.
  # Nushell's pre_execution hook receives no command text, so C carries no
  # payload — the daemon still flips to "command running" on the bare mark.
  $env.config.hooks.pre_execution = ($env.config.hooks.pre_execution | default [] | append {||
    $env.__lyrel_cmd_active = true
    print -n $"(ansi osc)133;C;;aid=lyrel(char bel)"
  })

  # OSC 133 prompt-end (B) must land after the very last prompt character, and
  # the only hook-shaped place Nushell runs at that point is prompt_indicator —
  # so wrap it, but only when the config defines one. A missing indicator is
  # the built-in default prompt's to render (recent Nushells draw it and their
  # own B mark themselves), and replacing it here would erase the glyph.
  if ((($env.config.prompt_indicator? | default null) | describe) != 'nothing') {
    let __lyrel_orig_indicator = $env.config.prompt_indicator
    $env.config.prompt_indicator = {||
      let __lyrel_ind = match ($__lyrel_orig_indicator | describe) {
        'closure' => (do $__lyrel_orig_indicator)
        _ => $__lyrel_orig_indicator
      }
      $"($__lyrel_ind)(ansi osc)133;B(char bel)"
    }
  }
}
# --- end lyrel shell integration ---
"#;

fn zsh_redirectors() -> [(&'static str, String); 4] {
    let redirect = |name: &str, tail: &str| {
        format!(
            "__lyrel_ztmp=$ZDOTDIR\n\
             if [[ -n \"$LYREL_USER_ZDOTDIR\" ]]; then ZDOTDIR=$LYREL_USER_ZDOTDIR; else unset ZDOTDIR; fi\n\
             [[ -f \"${{ZDOTDIR:-$HOME}}/{name}\" ]] && source \"${{ZDOTDIR:-$HOME}}/{name}\"\n\
             {tail}ZDOTDIR=$__lyrel_ztmp\n\
             unset __lyrel_ztmp\n"
        )
    };
    [
        (
            ".zshenv",
            redirect(".zshenv", "export LYREL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}\n"),
        ),
        (".zprofile", redirect(".zprofile", "")),
        (
            ".zshrc",
            format!("{}{ZSH_INTEGRATION}", redirect(".zshrc", "")),
        ),
        (".zlogin", redirect(".zlogin", "")),
    ]
}

pub struct Injection {
    pub env: HashMap<String, String>,
    pub args: Vec<String>,
    pub replaces_argv: bool,
    pub dir: Option<PathBuf>,
}

const ZDOTDIR_PREFIX: &str = "lyrel-zdotdir-";

fn is_our_zdotdir(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(ZDOTDIR_PREFIX))
}

fn real_user_zdotdir() -> Option<String> {
    if let Ok(z) = std::env::var("LYREL_USER_ZDOTDIR") {
        if !z.is_empty() {
            return Some(z);
        }
    }
    std::env::var("ZDOTDIR")
        .ok()
        .filter(|z| !z.is_empty() && !is_our_zdotdir(z))
}

enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
    Nushell,
    Wsl,
}

fn shell_kind(program: Option<&str>) -> Option<ShellKind> {
    let owned = match program {
        Some(p) => p.to_string(),
        None => crate::core::shells::login_shell(),
    };
    let base = Path::new(&owned)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    match base.strip_suffix(".exe").unwrap_or(&base) {
        "zsh" => Some(ShellKind::Zsh),
        "bash" if cfg!(windows) && !is_msys_bash(&owned) => None,
        "bash" => Some(ShellKind::Bash),
        "fish" => Some(ShellKind::Fish),
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        "nu" => Some(ShellKind::Nushell),
        "wsl" => Some(ShellKind::Wsl),
        _ => None,
    }
}

pub(crate) fn wsl_distro(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--distribution" || a == "-d" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--distribution=") {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn wslenv_with(existing: Option<&str>, additions: &[&str]) -> String {
    let mut out: Vec<String> = existing
        .unwrap_or("")
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for add in additions {
        let name = add.split('/').next().unwrap_or(add);
        if !out.iter().any(|e| e.split('/').next().unwrap_or(e) == name) {
            out.push((*add).to_string());
        }
    }
    out.join(":")
}

fn is_msys_bash(program: &str) -> bool {
    if !cfg!(windows) {
        return true;
    }
    let normalized = program.replace('/', "\\").to_ascii_lowercase();
    let Some((dir, _)) = normalized.rsplit_once('\\') else {
        return false;
    };
    let system_root = std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".to_string())
        .replace('/', "\\")
        .to_ascii_lowercase();
    let system_root = system_root.trim_end_matches('\\');
    !(dir == system_root || dir.starts_with(&format!("{system_root}\\")))
}

fn throwaway_dir(prefix: &str) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn setup_zsh() -> Option<Injection> {
    let dir = throwaway_dir(ZDOTDIR_PREFIX)?;
    for (name, contents) in zsh_redirectors() {
        std::fs::write(dir.join(name), contents).ok()?;
    }

    let mut env = HashMap::new();
    if let Some(user_zdotdir) = real_user_zdotdir() {
        env.insert("LYREL_USER_ZDOTDIR".to_string(), user_zdotdir);
    }
    env.insert("ZDOTDIR".to_string(), dir.to_string_lossy().into_owned());

    Some(Injection {
        env,
        args: Vec::new(),
        replaces_argv: false,
        dir: Some(dir),
    })
}

fn setup_fish() -> Option<Injection> {
    Some(Injection {
        env: HashMap::new(),
        args: vec!["-C".to_string(), FISH_INTEGRATION.to_string()],
        replaces_argv: false,
        dir: None,
    })
}

fn setup_powershell() -> Option<Injection> {
    Some(Injection {
        env: HashMap::new(),
        args: vec![
            "-NoLogo".to_string(),
            "-NoExit".to_string(),
            "-EncodedCommand".to_string(),
            powershell_encoded_command(POWERSHELL_INTEGRATION),
        ],
        replaces_argv: false,
        dir: None,
    })
}

/// Nushell has no environment variable that redirects its config file the way
/// ZDOTDIR does for zsh, so the injection rides `--config` instead: a
/// throwaway config.nu that sources the user's real one and then appends the
/// OSC hooks. The `dir` is what makes the file disappear when the pane closes.
///
/// Trade-off, inherent to `--config`: `$nu.config-path` points at the wrapper,
/// so `config nu` inside the pane edits a file that vanishes with it. The zsh
/// path dodges the analogue by restoring ZDOTDIR after startup; Nushell's
/// `$nu.*` paths are immutable, so this is accepted rather than fought.
fn setup_nushell() -> Option<Injection> {
    setup_nushell_with(nushell_user_config_path().as_deref())
}

/// Split from `setup_nushell` so a test can drive the whole wrapper against a
/// config.nu of its own without writing into the real one.
fn setup_nushell_with(user_config: Option<&Path>) -> Option<Injection> {
    let dir = throwaway_dir("lyrel-nu-")?;
    let config = dir.join("config.nu");
    std::fs::write(&config, nushell_config_script_with(user_config)).ok()?;

    Some(Injection {
        env: HashMap::new(),
        args: vec![
            "--config".to_string(),
            config.to_string_lossy().into_owned(),
        ],
        replaces_argv: false,
        dir: Some(dir),
    })
}

/// The wrapper config.nu: the user's real config.nu sourced back in when it
/// exists, then the OSC hooks. `source` is a parse-time construct in Nushell —
/// it cannot be guarded at runtime or name a missing file — so the path is
/// resolved and substituted here, and a machine without a config.nu gets a
/// no-op line instead.
fn nushell_config_script_with(user_config: Option<&Path>) -> String {
    let source = match user_config {
        Some(path) => format!("source {}", nu_string_literal(&path.to_string_lossy())),
        None => "# no user config.nu to restore".to_string(),
    };
    NUSHELL_INTEGRATION.replace("__LYREL_SOURCE_USER_CONFIG__", &source)
}

/// Where nu would load its config.nu from. Only a file that exists comes
/// back — the wrapper's `source` cannot name one that is not there.
fn nushell_user_config_path() -> Option<PathBuf> {
    let config = nushell_config_dir()?.join("config.nu");
    config.is_file().then_some(config)
}

/// nu's own config directory, resolved the way nu resolves it. Getting this
/// wrong is silent and total: the wrapper reports "no config.nu to restore"
/// for a user who has one, and `nu --config` then replaces their config with
/// one that never sources it.
///
/// nu-path's `configurable_dir_path` consults `$XDG_CONFIG_HOME` on *every*
/// platform, Windows included, and only when it is non-empty **and** absolute;
/// anything else falls through to `dirs::config_dir()`.
fn nushell_config_dir() -> Option<PathBuf> {
    nushell_config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        platform_config_dir(),
    )
}

fn nushell_config_dir_from(
    xdg_config_home: Option<&std::ffi::OsStr>,
    platform_default: Option<PathBuf>,
) -> Option<PathBuf> {
    let base = match xdg_config_home {
        Some(xdg) if !xdg.is_empty() && Path::new(xdg).is_absolute() => PathBuf::from(xdg),
        _ => platform_default?,
    };
    Some(base.join("nushell"))
}

/// `dirs::config_dir()` — what nu falls back to. Note this is **not** where
/// lyrel keeps its own config (`~/.config/lyrel` on macOS too); nu follows the
/// platform convention, and the two are only the same directory on Linux.
#[cfg(windows)]
fn platform_config_dir() -> Option<PathBuf> {
    // `dirs` asks Windows for FOLDERID_RoamingAppData; `%APPDATA%` is the
    // same directory in every environment lyrel can spawn a pane in.
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config"))
}

/// A Nushell string literal for `path`. Single quotes are fully literal in
/// Nushell, so they are the default; a path that contains an apostrophe (legal
/// on Windows) falls back to double quotes with backslash escaping.
fn nu_string_literal(path: &str) -> String {
    if !path.contains('\'') {
        return format!("'{path}'");
    }
    let escaped = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

fn powershell_encoded_command(script: &str) -> String {
    let utf16le: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64_encode(&utf16le)
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn bash_rcfile() -> String {
    format!(
        r#"
# Replays what a real *login* shell would have sourced, in the same order —
# necessary because lyrel spawns bash non-login (see shell_integration.rs) so
# that --rcfile is honored at all; bash silently ignores it for login shells.
#
# ~/.bashrc needs both halves of a rule that cannot be written as a static
# order, because the two shapes in the wild want opposite things:
#
#   * a ~/.bash_profile that forwards to ~/.bashrc (the common case, and what
#     every "put this in your .bash_profile" guide produces). Sourcing ~/.bashrc
#     again afterwards runs the user's whole file twice: banners print twice,
#     completions get sourced twice, anything appending to PROMPT_COMMAND stacks.
#   * a ~/.bash_profile that does *not* forward — written by someone whose
#     terminal spawns non-login shells, so ~/.bashrc always arrived on its own
#     and the profile only ever had to hold login-time settings. Dropping
#     ~/.bashrc from the chain would take that user's aliases, prompt and
#     completions away entirely.
#
# So watch instead of guess: wrap `source`/`.` for the length of the chain, note
# whether ~/.bashrc came through, and fill it in afterwards only if it did not.
# `~` is already expanded to $HOME by the time the wrapper sees $1, so matching
# on the trailing path component covers `. ~/.bashrc`, `source "$HOME/.bashrc"`
# and a spelled-out absolute path alike.
__lyrel_bashrc=0
source() {{ case "${{1-}}" in */.bashrc|.bashrc) __lyrel_bashrc=1;; esac; builtin source "$@"; }}
.() {{ case "${{1-}}" in */.bashrc|.bashrc) __lyrel_bashrc=1;; esac; builtin . "$@"; }}
if [[ -f /etc/profile ]]; then source /etc/profile; fi
if [[ -f ~/.bash_profile ]]; then
  source ~/.bash_profile
elif [[ -f ~/.bash_login ]]; then
  source ~/.bash_login
elif [[ -f ~/.profile ]]; then
  source ~/.profile
fi
unset -f source .
if [[ $__lyrel_bashrc == 0 && -f ~/.bashrc ]]; then source ~/.bashrc; fi
unset __lyrel_bashrc
{BASH_INTEGRATION}"#
    )
}

fn bash_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

fn setup_bash() -> Option<Injection> {
    let dir = throwaway_dir("lyrel-bashrc-")?;
    let rcfile = dir.join("bashrc");
    std::fs::write(&rcfile, bash_rcfile()).ok()?;

    Some(Injection {
        env: HashMap::new(),
        args: vec!["--rcfile".to_string(), bash_path(&rcfile), "-i".to_string()],
        replaces_argv: true,
        dir: Some(dir),
    })
}

const WSL_RCFILE_ENV: &str = "LYREL_RC";

/// Where the zsh redirectors landed, in the Windows spelling `WSLENV`'s `/p`
/// translates for the distro.
///
/// Deliberately not `ZDOTDIR`: everything in this map is handed to *every* WSL
/// pane, and a bash or fish distro that found a `ZDOTDIR` in its environment
/// would carry it into any `zsh` started inside it. The bootstrap decides
/// whether this becomes `ZDOTDIR`, and it only decides that for a zsh distro.
const WSL_ZDOTDIR_ENV: &str = "LYREL_ZDOTDIR";

/// POSIX single-quoting, for a body some other shell has to re-parse.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The bootstrap `sh` runs inside the distro. `$SHELL` is the only place the
/// user's real shell is named, so every arm dispatches on it: bash re-execs
/// through the rcfile written on the Windows side, zsh points `ZDOTDIR` at the
/// redirectors written beside it, fish carries its integration inline the way
/// `remote::bootstrap_command` does over SSH, and anything else falls back to a
/// plain login shell.
///
/// Both file-backed arms ask whether what they were handed is really there
/// before trusting it. The files live on the Windows side and reach the distro
/// over `/mnt`, which is not a given: automount can be off, `/etc/wsl.conf` can
/// move the root, and a distro can have no drvfs at all. Unguarded, bash is
/// handed a `--rcfile` it cannot read — which it ignores in silence, taking the
/// user's own `.bashrc` down with it, because lyrel starts it non-login so that
/// `--rcfile` is honoured at all — and zsh is pointed at a `ZDOTDIR` that does
/// not exist, which takes every startup file the user wrote with it. Guarded,
/// such a distro gets the plain login shell it had before any of this: no
/// integration, but the startup files a bare `wsl.exe` pane reads, which is the
/// failure worth having.
///
/// `sh` parses this script, so the fish body is POSIX-quoted here — unlike the
/// SSH path, where the far end's own login shell parses the bootstrap and
/// `remote::fish_bootstrap` has to quote it the way fish reads quotes.
#[cfg_attr(not(windows), allow(dead_code))]
fn wsl_exec_script() -> String {
    format!(
        concat!(
            r#"case "${{SHELL:-}}" in "#,
            r#"*/bash) [ -r "${{LYREL_RC:-}}" ] && exec "$SHELL" --rcfile "$LYREL_RC" -i; "#,
            r#"exec "$SHELL" -l ;; "#,
            r#"*/zsh) [ -n "${{LYREL_ZDOTDIR:-}}" ] && [ -r "$LYREL_ZDOTDIR/.zshrc" ] "#,
            r#"&& export ZDOTDIR="$LYREL_ZDOTDIR"; exec "$SHELL" -l ;; "#,
            r#"*/fish) exec "$SHELL" -C {} -l ;; "#,
            r#"*) exec "${{SHELL:-/bin/sh}}" -l ;; "#,
            "esac"
        ),
        shell_quote(FISH_INTEGRATION)
    )
}

/// The files a WSL pane needs on the Windows side, and the environment that
/// tells the distro where they are.
///
/// Split out of `setup_wsl` so that everything deciding anything is compiled
/// and tested on every platform; what stays behind the `#[cfg(windows)]` is the
/// argv and a call to this.
///
/// `LYREL_USER_ZDOTDIR` is conspicuously absent, and that is the point.
/// `real_user_zdotdir` answers from this process's own environment, and this
/// process is on the Windows side of the boundary — the `ZDOTDIR` the user
/// actually set lives inside the distro and is a Linux path nothing out here
/// can name. A Windows path sent across would aim every redirector at a
/// directory that is not there, and the user's `.zshrc` would go missing
/// silently. Left unset, `zsh_redirectors` falls through to `${ZDOTDIR:-$HOME}`
/// — the distro's own home — and its `.zshenv` arm recaptures a relocated
/// `ZDOTDIR` from in there, which is the only side that ever knew it.
#[cfg_attr(not(windows), allow(dead_code))]
fn wsl_integration_env(dir: &Path, wslenv: Option<&str>) -> Option<HashMap<String, String>> {
    let rcfile = dir.join("bashrc");
    std::fs::write(&rcfile, bash_rcfile()).ok()?;

    let mut env = HashMap::new();
    let mut names = vec![format!("{WSL_RCFILE_ENV}/p")];
    env.insert(
        WSL_RCFILE_ENV.to_string(),
        rcfile.to_string_lossy().into_owned(),
    );

    // The zsh half is best-effort on purpose. It is the bash rcfile that a WSL
    // pane has depended on since this path existed, and failing the whole setup
    // because a second set of files could not be written would take a working
    // bash distro down with it. Nothing names the directory unless all of it
    // landed, so a half-written one is never advertised to the distro.
    if let Some(zdotdir) = wsl_zdotdir(dir) {
        env.insert(WSL_ZDOTDIR_ENV.to_string(), zdotdir);
        names.push(format!("{WSL_ZDOTDIR_ENV}/p"));
    }

    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    env.insert("WSLENV".to_string(), wslenv_with(wslenv, &names));
    Some(env)
}

/// The zsh redirectors, under the pane's own throwaway directory so that the
/// `remove_dir_all` closing the pane already takes them.
#[cfg_attr(not(windows), allow(dead_code))]
fn wsl_zdotdir(dir: &Path) -> Option<String> {
    let zdotdir = dir.join(format!("{ZDOTDIR_PREFIX}wsl"));
    std::fs::create_dir_all(&zdotdir).ok()?;
    for (name, contents) in zsh_redirectors() {
        std::fs::write(zdotdir.join(name), contents).ok()?;
    }
    Some(zdotdir.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn setup_wsl(args: &[String]) -> Option<Injection> {
    let distro = wsl_distro(args);
    let dir = throwaway_dir("lyrel-wslrc-")?;
    let env = wsl_integration_env(&dir, std::env::var("WSLENV").ok().as_deref())?;

    let mut argv: Vec<String> = Vec::new();
    if let Some(d) = &distro {
        argv.push("--distribution".to_string());
        argv.push(d.clone());
    }
    if let Some(cd) = wsl_cd(args) {
        argv.push("--cd".to_string());
        argv.push(cd);
    }
    // `--` forwards the command through the distro's default shell, which makes
    // shells such as fish parse the POSIX bootstrap before `sh` can receive it.
    // `--exec` bypasses that shell and executes the bootstrap interpreter itself.
    argv.push("--exec".to_string());
    argv.push("sh".to_string());
    argv.push("-c".to_string());
    argv.push(wsl_exec_script());

    Some(Injection {
        env,
        args: argv,
        replaces_argv: true,
        dir: Some(dir),
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn wsl_cd(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--cd" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--cd=") {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg_attr(not(windows), allow(unused_variables))]
pub fn setup(program: Option<&str>, args: &[String], has_custom_args: bool) -> Option<Injection> {
    // Every shell defers to user-authored args, so the gate sits ahead of the
    // dispatch rather than once per arm — a shell added below inherits it
    // instead of having to remember it. Argv injection would collide with those
    // args outright, and even zsh's env-only ZDOTDIR swap changes which startup
    // files run. Arguments lyrel's own detection supplied (Git Bash's `-i -l`,
    // a WSL row's `--distribution`) are not user-authored and never land here;
    // `daemon::pane::has_custom_args` is where that line is drawn.
    if has_custom_args {
        return None;
    }
    let mut injection = match shell_kind(program)? {
        ShellKind::Zsh => setup_zsh(),
        ShellKind::Fish => setup_fish(),
        ShellKind::Bash => setup_bash(),
        ShellKind::PowerShell => setup_powershell(),
        ShellKind::Nushell => setup_nushell(),
        #[cfg(windows)]
        ShellKind::Wsl => setup_wsl(args),
        #[cfg(not(windows))]
        ShellKind::Wsl => None,
    }?;

    injection
        .env
        .insert("LYREL_SHELL_INTEGRATION".to_string(), String::new());

    Some(injection)
}

pub mod remote {
    use super::{
        FISH_INTEGRATION, POWERSHELL_INTEGRATION, bash_rcfile, powershell_encoded_command,
        shell_quote, zsh_redirectors,
    };

    /// Asks the far host which shell it runs, and is written to be read by
    /// every shell sshd might hand it to — the exec request runs through the
    /// account's default shell, and on Windows OpenSSH that is `cmd.exe /c`,
    /// `powershell.exe -c` or `pwsh -c`, not a POSIX `-c`. So it sticks to
    /// words all of them read without complaint, and each answers differently:
    ///
    /// - a POSIX shell (and fish) prints the marker, `$SHELL`, and the two
    ///   `__lyrel_ps*=` keys with nothing after the `=` (fish drops the word
    ///   outright and prints an empty line);
    /// - PowerShell has no `$SHELL` variable, so that `echo` prints nothing,
    ///   and fills the keys from its own automatic variables — the edition
    ///   (`Desktop` is Windows PowerShell 5.1, `Core` is pwsh) and the
    ///   directory its executable lives in;
    /// - cmd.exe has no `;` separator: it echoes the whole line back verbatim,
    ///   marker and all, on one line.
    ///
    /// The shape of each word is load-bearing. No quotes: Windows OpenSSH
    /// hands the command to the shell through a Win32 command line, and `"`s
    /// there are argv syntax, gone before PowerShell reads the text. No
    /// parentheses or pipes: a POSIX shell parses the whole line before
    /// running any of it, so one construct it rejects would cost the answer
    /// it does give. `key=$VAR` expands in every dialect that expands at all
    /// (PowerShell's argument mode included), the `=` ending the key.
    pub const PROBE_COMMAND: &str = "echo __lyrel_shell; echo $SHELL; \
         echo __lyrel_psedition=$PSEdition; echo __lyrel_pshome=$PSHOME";

    const PROBE_MARKER: &str = "__lyrel_shell";

    const PS_EDITION_KEY: &str = "__lyrel_psedition=";

    const PS_HOME_KEY: &str = "__lyrel_pshome=";

    const HEREDOC: &str = "__LYREL_RC_EOF__";

    /// The longest bootstrap sent to a PowerShell host. Windows caps a
    /// process command line at 32,767 UTF-16 units, and the bootstrap rides
    /// inside two of them: sshd's `<shell> -c <bootstrap>`, then that shell's
    /// launch of the interactive PowerShell, which is a little shorter. The
    /// headroom is for sshd's part — the shell's path and switch — with room
    /// to spare for any quoting it adds. cmd.exe's far smaller 8,191 never
    /// applies: nothing is sent through cmd.
    const POWERSHELL_COMMAND_LINE_BUDGET: usize = 30_000;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RemoteShell {
        Zsh,
        Bash,
        Fish,
        /// Windows PowerShell or pwsh as a Windows account's default shell.
        /// The path the probe pairs with it is the executable, `C:\…\pwsh.exe`.
        PowerShell,
    }

    impl RemoteShell {
        fn from_path(path: &str) -> Option<Self> {
            match path.rsplit('/').next()? {
                "zsh" => Some(Self::Zsh),
                "bash" => Some(Self::Bash),
                "fish" => Some(Self::Fish),
                _ => None,
            }
        }
    }

    pub fn parse_probe(output: &str) -> Option<(RemoteShell, String)> {
        let mut lines = output
            .lines()
            .map(|l| l.trim_end_matches('\r').trim())
            .skip_while(|l| *l != PROBE_MARKER);
        lines.next()?;
        let lines: Vec<&str> = lines.collect();
        if let Some(exe) = powershell_from_probe(&lines) {
            return Some((RemoteShell::PowerShell, exe));
        }
        let path = *lines.iter().find(|l| !l.is_empty())?;
        if !path.starts_with('/') {
            return None;
        }
        RemoteShell::from_path(path).map(|shell| (shell, path.to_string()))
    }

    /// Whether the probe was answered by cmd.exe — Windows OpenSSH's default
    /// shell out of the box. Such a host is left a plain shell, on purpose:
    /// cmd has no hook that could carry the integration, and starting
    /// PowerShell where the account asked for cmd would change the user's
    /// shell rather than instrument it. A host whose `DefaultShell` names
    /// PowerShell gets it. Only for the log, so the absence explains itself.
    pub fn probe_answered_by_cmd(output: &str) -> bool {
        let echoed = format!("{PROBE_MARKER};");
        output.lines().any(|l| l.trim().starts_with(&echoed))
    }

    /// The PowerShell executable a probe describes, if PowerShell answered.
    ///
    /// The edition names the binary — `powershell.exe` is only ever Windows
    /// PowerShell, `pwsh` only ever Core — and `$PSHOME` is the directory it
    /// sits in, so the path is exactly the shell that ran the probe: the one
    /// the account is configured with, whatever other edition is installed
    /// beside it. A Windows host with pwsh installed but Windows PowerShell as
    /// its default stays on Windows PowerShell, just as a plain shell request
    /// would have left it.
    ///
    /// Windows hosts only. pwsh as a Unix login shell answers the probe too,
    /// but there the binary in `$PSHOME` is not always runnable on its own —
    /// a package manager's install launches it through a wrapper that sets up
    /// the .NET runtime first, and the wrapper is what `$SHELL` names, where
    /// the probe cannot read it — so such a host keeps its plain shell.
    fn powershell_from_probe(lines: &[&str]) -> Option<String> {
        let field = |key: &str| lines.iter().find_map(|l| l.strip_prefix(key));
        let edition = field(PS_EDITION_KEY)?;
        let home = field(PS_HOME_KEY)?.trim_end_matches(['/', '\\']);
        if !is_windows_path(home) || home.contains('\0') {
            return None;
        }
        match edition {
            "Desktop" => Some(format!(r"{home}\powershell.exe")),
            "Core" => Some(format!(r"{home}\pwsh.exe")),
            _ => None,
        }
    }

    /// A drive (`C:\…`, `C:/…`) or UNC (`\\server\…`) path.
    fn is_windows_path(path: &str) -> bool {
        let b = path.as_bytes();
        (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || path.starts_with(r"\\")
    }

    /// The script a native SSH session execs in place of a bare shell request.
    ///
    /// `start_dir` is a directory *on the remote host* — where the pane was the
    /// last time the far shell reported its cwd — and the session moves there
    /// before the user's shell is exec'd, so a redialled pane comes back where
    /// it was instead of in the login directory. It rides in the bootstrap
    /// rather than being typed at the prompt so it never shows on screen or
    /// lands in history, and a directory that has gone away since fails the
    /// `cd` quietly and leaves the session in the login directory, exactly as
    /// if nothing had been asked.
    ///
    /// The `cd` is the script's first line, ahead of the rc staging: a
    /// relative `$TMPDIR` then resolves against one directory for every line
    /// that follows, not one before the `cd` and another after it.
    pub fn bootstrap_command(
        shell: RemoteShell,
        shell_path: &str,
        start_dir: Option<&str>,
    ) -> String {
        if shell == RemoteShell::PowerShell {
            return powershell_bootstrap(shell_path, start_dir);
        }
        let mut out = start_dir
            .and_then(usable_start_dir)
            .map(|dir| cd_line(shell, dir))
            .unwrap_or_default();
        out.push_str(&match shell {
            RemoteShell::Zsh => zsh_bootstrap(shell_path),
            RemoteShell::Bash => bash_bootstrap(shell_path),
            RemoteShell::Fish => fish_bootstrap(shell_path),
            RemoteShell::PowerShell => unreachable!("returned above"),
        });
        out
    }

    /// `dir`, if it can only mean one place on the far host.
    ///
    /// Absolute paths only: a relative one would resolve against the login
    /// directory and through `CDPATH`, and a `~` would reach the shell quoted
    /// and never expand. Neither is anything OSC 7 reports, so honouring one
    /// would be a guess. A NUL cannot travel in an exec request at all.
    fn usable_start_dir(dir: &str) -> Option<&str> {
        (dir.starts_with('/') && !dir.contains('\0')).then_some(dir)
    }

    /// `builtin`, so a `cd` wrapper defined in a file the `-c` shell already
    /// read (`.zshenv`, fish's `config.fish`) cannot print, prompt or refuse;
    /// `--`, so no directory name is ever taken for an option.
    fn cd_line(shell: RemoteShell, dir: &str) -> String {
        let quoted = match shell {
            RemoteShell::Zsh | RemoteShell::Bash => shell_quote(dir),
            RemoteShell::Fish => fish_quote(dir),
            RemoteShell::PowerShell => unreachable!("PowerShell moves with Set-Location"),
        };
        format!("builtin cd -- {quoted} 2>/dev/null\n")
    }

    /// The command the host's default PowerShell runs: start a second,
    /// interactive PowerShell — the same executable — carrying the
    /// integration.
    ///
    /// A second process because Windows has no `exec`: the shell sshd started
    /// was told `-c`, which runs one command and leaves, and nothing can turn
    /// it interactive after the fact. It waits on the child and hands back its
    /// exit code, so the session ends when the user's shell does.
    ///
    /// The child is started the way a local PowerShell pane is (`-NoLogo
    /// -NoExit -EncodedCommand`), so the user's profile loads first and the
    /// integration wraps whatever prompt it settled on. The start directory
    /// rides inside the encoded script too — base64 carries any path through
    /// the Win32 command line untouched, where a literal would be at the
    /// mercy of sshd's quoting — which puts it after the profile: a profile
    /// that moves somewhere is overruled for a redialled pane, which is the
    /// point of asking for a directory at all. One that has gone away fails
    /// quietly into wherever the profile left the session.
    fn powershell_bootstrap(exe: &str, start_dir: Option<&str>) -> String {
        let build = |dir: Option<&str>| {
            let mut script = dir.map(set_location_line).unwrap_or_default();
            script.push_str(&compact_powershell(POWERSHELL_INTEGRATION));
            format!(
                "& {} -NoLogo -NoExit -EncodedCommand {}; exit $LASTEXITCODE",
                ps_quote(exe),
                powershell_encoded_command(&script)
            )
        };
        let dir = start_dir.and_then(powershell_start_dir);
        let with_dir = build(dir.as_deref());
        // A start directory long enough to push the command line past what
        // Windows will run is dropped rather than costing the session: the
        // pane opens where the login leaves it, still integrated.
        if dir.is_some() && with_dir.chars().count() > POWERSHELL_COMMAND_LINE_BUDGET {
            return build(None);
        }
        with_dir
    }

    /// `dir` as a path the far Windows host can only read as one place.
    ///
    /// The integration reports its cwd the way a `file://` URI spells a drive
    /// path — `/C:/Users/ann` — and the local side may have kept it that way
    /// (a Unix lyrel) or already unwrapped it to `C:/Users/ann` (a Windows
    /// one); both mean drive C. A UNC share comes back as `//server/share/…`.
    /// Anything else is refused as ambiguous: `/Users/ann` is relative to
    /// whichever drive is current, and a bare `C:` means wherever drive C
    /// last was.
    fn powershell_start_dir(dir: &str) -> Option<String> {
        if dir.contains('\0') {
            return None;
        }
        let drive = |b: &[u8]| {
            b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'/' | b'\\')
        };
        if drive(dir.as_bytes()) {
            return Some(dir.to_string());
        }
        if let Some(rest) = dir.strip_prefix('/')
            && drive(rest.as_bytes())
        {
            return Some(rest.to_string());
        }
        let unc = dir.replace('/', r"\");
        let (server, rest) = unc.strip_prefix(r"\\")?.split_once('\\')?;
        let share = rest.split('\\').next().unwrap_or_default();
        // `\\?\` and `\\.\` are device namespaces, not a server's shares.
        let named = |s: &str| !s.is_empty() && s != "?" && s != ".";
        (named(server) && named(share)).then_some(unc)
    }

    /// `-LiteralPath`, so brackets and `*` in a directory name are only
    /// characters; a directory that is gone is caught rather than reported,
    /// and `Stop` makes sure it is catchable — without it, a missing path is a
    /// non-terminating error that prints straight past the `try`.
    fn set_location_line(dir: &str) -> String {
        format!(
            "try {{ Set-Location -LiteralPath {} -ErrorAction Stop }} catch {{ }}\n",
            ps_quote(dir)
        )
    }

    /// PowerShell single-quoting. PowerShell takes the typographic single
    /// quotes for quote marks as well, so they are doubled along with `'`.
    fn ps_quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for c in s.chars() {
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                out.push(c);
            }
            out.push(c);
        }
        out.push('\'');
        out
    }

    /// The integration with its comments, blank lines and indentation
    /// dropped. It travels as base64 of UTF-16 — each character costs 8/3 of
    /// one on the command line — and the commentary is half of it. The script
    /// has no here-strings and no string that breaks onto a line starting
    /// with `#`, so such a line is always a comment.
    fn compact_powershell(script: &str) -> String {
        let mut out = String::with_capacity(script.len());
        for line in script.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    fn fish_quote(s: &str) -> String {
        format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
    }

    fn write_file(out: &mut String, name: &str, body: &str) {
        out.push_str(&format!(
            "command cat > \"$__lyrel_d/{name}\" <<'{HEREDOC}'\n{}\n{HEREDOC}\n",
            body.trim_end_matches('\n')
        ));
    }

    fn zsh_bootstrap(shell_path: &str) -> String {
        let mut out = String::new();
        out.push_str("__lyrel_d=${TMPDIR:-/tmp}/lyrel-zdotdir-$$\n");
        out.push_str("command mkdir -p \"$__lyrel_d\" 2>/dev/null\n");

        let mut guard = String::new();
        for (name, contents) in zsh_redirectors() {
            let body = if name == ".zshrc" {
                format!("{contents}{ZSH_CLEANUP_HOOK}")
            } else {
                contents
            };
            write_file(&mut out, name, &body);
            guard.push_str(&format!("[ -s \"$__lyrel_d/{name}\" ] && "));
        }
        out.push_str(&format!(
            "{guard}export ZDOTDIR=\"$__lyrel_d\" LYREL_RM_DIR=\"$__lyrel_d\"\n"
        ));
        out.push_str(&format!("exec {} -l\n", shell_quote(shell_path)));
        out
    }

    fn bash_bootstrap(shell_path: &str) -> String {
        let quoted = shell_quote(shell_path);
        let mut out = String::new();
        out.push_str("__lyrel_d=${TMPDIR:-/tmp}/lyrel-bashrc-$$\n");
        out.push_str("command mkdir -p \"$__lyrel_d\" 2>/dev/null\n");
        write_file(
            &mut out,
            "bashrc",
            &format!("{}{BASH_CLEANUP_HOOK}", bash_rcfile()),
        );
        out.push_str("if [ -s \"$__lyrel_d/bashrc\" ]; then\n");
        out.push_str("export LYREL_RM_DIR=\"$__lyrel_d\"\n");
        out.push_str(&format!(
            "exec {quoted} --rcfile \"$__lyrel_d/bashrc\" -i\n"
        ));
        out.push_str("fi\n");
        out.push_str(&format!("exec {quoted} -l\n"));
        out
    }

    fn fish_bootstrap(shell_path: &str) -> String {
        format!(
            "exec {} -C {} -l\n",
            fish_quote(shell_path),
            fish_quote(FISH_INTEGRATION)
        )
    }

    const ZSH_CLEANUP_HOOK: &str = r#"
# --- lyrel remote cleanup (zsh) ---
if [[ -n "$LYREL_RM_DIR" ]]; then
  typeset -g __lyrel_rm_dir=$LYREL_RM_DIR
  unset LYREL_RM_DIR
  __lyrel_rm_rcdir() {
    [[ -n "$__lyrel_rm_dir" ]] || return 0
    command rm -rf -- "$__lyrel_rm_dir"
    unset __lyrel_rm_dir
  }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __lyrel_rm_rcdir
fi
# --- end lyrel remote cleanup ---
"#;

    pub(super) const BASH_CLEANUP_HOOK: &str = r#"
# --- lyrel remote cleanup (bash) ---
if [[ -n "$LYREL_RM_DIR" ]]; then
  __lyrel_rm_dir=$LYREL_RM_DIR
  unset LYREL_RM_DIR
  __lyrel_rm_rcdir() {
    [[ -n "$__lyrel_rm_dir" ]] || return 0
    command rm -rf -- "$__lyrel_rm_dir"
    unset __lyrel_rm_dir
  }
fi
# --- end lyrel remote cleanup ---
"#;

    #[cfg(test)]
    mod tests {
        use super::super::{BASH_INTEGRATION, ZSH_INTEGRATION};
        use super::*;

        #[test]
        fn probe_reads_the_line_after_the_marker() {
            assert_eq!(
                parse_probe("__lyrel_shell\n/bin/zsh\n"),
                Some((RemoteShell::Zsh, "/bin/zsh".to_string()))
            );
            assert_eq!(
                parse_probe("Welcome to prod!\n__lyrel_shell\n/usr/local/bin/fish\n"),
                Some((RemoteShell::Fish, "/usr/local/bin/fish".to_string()))
            );
            assert_eq!(
                parse_probe("__lyrel_shell\r\n/bin/bash\r\n"),
                Some((RemoteShell::Bash, "/bin/bash".to_string()))
            );
        }

        #[test]
        fn probe_declines_anything_that_isnt_a_shell_we_know() {
            assert_eq!(parse_probe("__lyrel_shell\n$SHELL\n"), None);
            assert_eq!(parse_probe("__lyrel_shell\n\n"), None);
            assert_eq!(parse_probe("__lyrel_shell\n/bin/ksh\n"), None);
            assert_eq!(parse_probe("/bin/zsh\n"), None);
        }

        #[test]
        fn zsh_bootstrap_gates_zdotdir_on_every_redirector_landing() {
            let script = bootstrap_command(RemoteShell::Zsh, "/bin/zsh", None);
            for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
                assert!(
                    script.contains(&format!("[ -s \"$__lyrel_d/{name}\" ] &&")),
                    "missing landing check for {name}"
                );
            }
            let export = script.find("export ZDOTDIR=").expect("exports ZDOTDIR");
            let exec = script.find("exec '/bin/zsh' -l").expect("execs zsh");
            assert!(export < exec);
            assert!(script.contains("__lyrel_report_cwd"));
        }

        #[test]
        fn file_writing_bootstraps_end_in_a_bare_exec_of_the_users_shell() {
            for (shell, path) in [
                (RemoteShell::Zsh, "/bin/zsh"),
                (RemoteShell::Bash, "/bin/bash"),
            ] {
                let script = bootstrap_command(shell, path, None);
                let last = script.trim_end().lines().last().unwrap();
                assert_eq!(
                    last,
                    format!("exec '{path}' -l"),
                    "{shell:?} bootstrap must end by exec'ing {path} bare"
                );
            }
        }

        #[test]
        fn bash_bootstrap_forces_a_non_login_shell_through_the_rcfile() {
            let script = bootstrap_command(RemoteShell::Bash, "/bin/bash", None);
            assert!(script.contains("exec '/bin/bash' --rcfile \"$__lyrel_d/bashrc\" -i"));
            assert!(script.contains("source /etc/profile"));
        }

        #[test]
        fn fish_bootstrap_is_one_exec_carrying_the_escaped_body() {
            let script = bootstrap_command(RemoteShell::Fish, "/usr/bin/fish", None);
            assert!(script.starts_with("exec '/usr/bin/fish' -C '"));
            assert!(script.trim_end().ends_with("' -l"));
            assert!(!script.contains("mkdir"));

            assert!(script.contains(r"printf \'\\e]%s\\a\' $argv[1]"));
        }

        #[test]
        fn quoting_survives_paths_and_bodies_that_fight_back() {
            assert_eq!(shell_quote("/o'dd/zsh"), r"'/o'\''dd/zsh'");
            assert_eq!(fish_quote(r"a'b\c"), r"'a\'b\\c'");
        }

        #[test]
        fn heredoc_delimiter_cannot_appear_in_a_body_it_delimits() {
            for body in [ZSH_INTEGRATION, BASH_INTEGRATION, FISH_INTEGRATION] {
                assert!(!body.contains(HEREDOC));
            }
            assert!(!bash_rcfile().contains(HEREDOC));
        }

        #[cfg(unix)]
        fn parse_check(
            shell: &str,
            syntax_only_flag: &str,
            script: &str,
        ) -> Option<(bool, String)> {
            use std::io::Write as _;
            use std::process::{Command, Stdio};

            let mut child = Command::new(shell)
                .arg(syntax_only_flag)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .ok()?;
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(script.as_bytes())
                .expect("write script");
            let out = child.wait_with_output().expect("wait for parse check");
            Some((
                out.status.success(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ))
        }

        #[cfg(unix)]
        #[test]
        fn bootstrap_scripts_parse_under_their_real_shells() {
            let cases = [
                (RemoteShell::Zsh, "zsh", "-n", "/bin/zsh"),
                (RemoteShell::Bash, "bash", "-n", "/bin/bash"),
                (RemoteShell::Fish, "fish", "--no-execute", "/usr/bin/fish"),
            ];
            for (shell, bin, flag, path) in cases {
                for start_dir in [None, Some(AWKWARD_DIR)] {
                    let script = bootstrap_command(shell, path, start_dir);
                    if let Some((ok, stderr)) = parse_check(bin, flag, &script) {
                        assert!(
                            ok,
                            "{bin} rejected its bootstrap script (start dir {start_dir:?}):\n{stderr}"
                        );
                    }
                }
            }
        }

        /// A remote directory with everything in it that quoting has to get
        /// right: a space, a single quote, a backslash and a `$`.
        const AWKWARD_DIR: &str = r"/srv/my service/it's \n $HOME";

        #[test]
        fn a_start_dir_opens_every_bootstrap_with_one_quiet_cd() {
            for (shell, path, expected) in [
                (
                    RemoteShell::Zsh,
                    "/bin/zsh",
                    r"builtin cd -- '/srv/my service/it'\''s \n $HOME' 2>/dev/null",
                ),
                (
                    RemoteShell::Bash,
                    "/bin/bash",
                    r"builtin cd -- '/srv/my service/it'\''s \n $HOME' 2>/dev/null",
                ),
                (
                    RemoteShell::Fish,
                    "/usr/bin/fish",
                    r"builtin cd -- '/srv/my service/it\'s \\n $HOME' 2>/dev/null",
                ),
            ] {
                let script = bootstrap_command(shell, path, Some(AWKWARD_DIR));
                let (first, rest) = script.split_once('\n').expect("more than one line");
                assert_eq!(first, expected, "{shell:?}");
                assert_eq!(
                    rest,
                    bootstrap_command(shell, path, None),
                    "{shell:?}: the cd is added in front, and nothing else changes"
                );
            }
        }

        #[test]
        fn without_a_start_dir_the_bootstrap_has_no_cd_at_all() {
            for (shell, path) in [
                (RemoteShell::Zsh, "/bin/zsh"),
                (RemoteShell::Bash, "/bin/bash"),
                (RemoteShell::Fish, "/usr/bin/fish"),
            ] {
                assert!(!bootstrap_command(shell, path, None).contains("builtin cd"));
            }
        }

        #[test]
        fn a_start_dir_that_is_not_an_absolute_path_is_ignored() {
            for dir in ["", "~/my_service", "my_service", "./x", "/a\0b"] {
                let script = bootstrap_command(RemoteShell::Bash, "/bin/bash", Some(dir));
                assert_eq!(
                    script,
                    bootstrap_command(RemoteShell::Bash, "/bin/bash", None),
                    "{dir:?} must not be turned into a cd"
                );
            }
        }

        /// Runs the bootstrap's own `cd` line, then `pwd`, in `bin` the way
        /// sshd runs an exec request: `<shell> -c <script>`, started in the
        /// login directory. `None` when `bin` is not installed here.
        #[cfg(unix)]
        fn land(
            shell: RemoteShell,
            bin: &str,
            home: &std::path::Path,
            start_dir: &str,
        ) -> Option<std::process::Output> {
            use std::process::Command;
            let script = format!("{}pwd\n", cd_line(shell, start_dir));
            Command::new(bin)
                .arg("-c")
                .arg(script)
                .current_dir(home)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", home)
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("XDG_DATA_HOME", home.join(".local/share"))
                .output()
                .ok()
        }

        #[cfg(unix)]
        #[test]
        fn a_real_shell_lands_in_the_start_dir_and_falls_back_home_without_a_word() {
            let root = tempfile::tempdir().expect("tempdir");
            // Canonical, so `pwd` has no symlinked `/var` to disagree about.
            let root = root.path().canonicalize().expect("canonical tempdir");
            let home = root.join("home");
            let target = root.join(r"my service/it's \n $HOME");
            std::fs::create_dir_all(&home).expect("home");
            std::fs::create_dir_all(&target).expect("target");
            let target_str = target.to_str().expect("utf-8 temp path");
            let missing = root.join("gone since");
            let missing_str = missing.to_str().expect("utf-8 temp path");

            for (shell, bin) in [
                (RemoteShell::Bash, "bash"),
                (RemoteShell::Zsh, "zsh"),
                (RemoteShell::Fish, "fish"),
            ] {
                let Some(out) = land(shell, bin, &home, target_str) else {
                    continue;
                };
                assert!(out.status.success(), "{bin}: {out:?}");
                assert_eq!(
                    String::from_utf8_lossy(&out.stdout).trim_end(),
                    target_str,
                    "{bin} did not land in the start dir"
                );

                let out = land(shell, bin, &home, missing_str).expect("ran once already");
                assert!(out.status.success(), "{bin}: {out:?}");
                assert_eq!(
                    String::from_utf8_lossy(&out.stdout).trim_end(),
                    home.to_str().unwrap(),
                    "{bin} left the login directory for a directory that is gone"
                );
                assert!(
                    out.stderr.is_empty(),
                    "{bin} complained about the missing directory: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn heredoc_bodies_parse_under_their_real_shells() {
            for (name, contents) in zsh_redirectors() {
                let body = if name == ".zshrc" {
                    format!("{contents}{ZSH_CLEANUP_HOOK}")
                } else {
                    contents
                };
                if let Some((ok, stderr)) = parse_check("zsh", "-n", &body) {
                    assert!(ok, "zsh rejected the remote {name}:\n{stderr}");
                }
            }
            let rcfile = format!("{}{BASH_CLEANUP_HOOK}", bash_rcfile());
            if let Some((ok, stderr)) = parse_check("bash", "-n", &rcfile) {
                assert!(ok, "bash rejected the remote rcfile:\n{stderr}");
            }
        }

        // --- Windows OpenSSH hosts ---

        const WIN_PS51: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        const WIN_PWSH: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";

        #[test]
        fn probe_names_the_powershell_that_answered_it() {
            // What each edition prints for PROBE_COMMAND: no line at all for
            // `echo $SHELL`, then the two keys. Windows output is CRLF.
            assert_eq!(
                parse_probe(
                    "__lyrel_shell\r\n__lyrel_psedition=Desktop\r\n\
                     __lyrel_pshome=C:\\Windows\\System32\\WindowsPowerShell\\v1.0\r\n"
                ),
                Some((RemoteShell::PowerShell, WIN_PS51.to_string()))
            );
            assert_eq!(
                parse_probe(
                    "__lyrel_shell\r\n__lyrel_psedition=Core\r\n\
                     __lyrel_pshome=C:\\Program Files\\PowerShell\\7\\\r\n"
                ),
                Some((RemoteShell::PowerShell, WIN_PWSH.to_string()))
            );
            // A profile that talks on the way in.
            assert_eq!(
                parse_probe(
                    "Loading personal and system profiles took 412ms.\r\n__lyrel_shell\r\n\
                     __lyrel_psedition=Core\r\n__lyrel_pshome=D:\\tools\\pwsh\r\n"
                ),
                Some((
                    RemoteShell::PowerShell,
                    r"D:\tools\pwsh\pwsh.exe".to_string()
                ))
            );
        }

        #[test]
        fn probe_declines_powershell_answers_it_cannot_place() {
            for out in [
                // pwsh as a Unix login shell keeps its plain shell.
                "__lyrel_shell\n__lyrel_psedition=Core\n__lyrel_pshome=/opt/microsoft/powershell/7\n",
                "__lyrel_shell\n__lyrel_psedition=Desktop\n__lyrel_pshome=/opt/ps\n",
                "__lyrel_shell\n__lyrel_psedition=Core\n__lyrel_pshome=\n",
                "__lyrel_shell\n__lyrel_psedition=Classic\n__lyrel_pshome=C:\\ps\n",
                "__lyrel_shell\n__lyrel_psedition=Core\n__lyrel_pshome=relative\\ps\n",
                // Keys before the marker are someone's banner, not an answer.
                "__lyrel_psedition=Core\n__lyrel_pshome=C:\\ps\n__lyrel_shell\n",
            ] {
                assert_eq!(parse_probe(out), None, "{out:?}");
            }
        }

        #[test]
        fn cmd_answers_the_probe_with_its_own_echo_and_is_left_alone() {
            // cmd has no `;`: the first `echo` prints the rest of the line.
            let echoed = PROBE_COMMAND.strip_prefix("echo ").unwrap();
            let out = format!("{echoed}\r\n");
            assert_eq!(parse_probe(&out), None);
            assert!(probe_answered_by_cmd(&out));
            // sshd may hand cmd the command in quotes; cmd echoes the tail one.
            assert!(probe_answered_by_cmd(&format!("{echoed}\"\r\n")));

            assert!(!probe_answered_by_cmd("__lyrel_shell\n/bin/zsh\n"));
            assert!(!probe_answered_by_cmd(
                "__lyrel_shell\n__lyrel_psedition=Core\n__lyrel_pshome=C:\\ps\n"
            ));
        }

        #[test]
        fn posix_answers_are_read_as_before_with_the_empty_keys_trailing() {
            assert_eq!(
                parse_probe("__lyrel_shell\n/bin/zsh\n__lyrel_psedition=\n__lyrel_pshome=\n"),
                Some((RemoteShell::Zsh, "/bin/zsh".to_string()))
            );
            // fish drops a word whose variable is unset: two empty lines.
            assert_eq!(
                parse_probe("__lyrel_shell\n/usr/bin/fish\n\n\n"),
                Some((RemoteShell::Fish, "/usr/bin/fish".to_string()))
            );
            // An unset $SHELL must not promote a key line to the shell path.
            assert_eq!(
                parse_probe("__lyrel_shell\n\n__lyrel_psedition=\n__lyrel_pshome=\n"),
                None
            );
        }

        /// Runs PROBE_COMMAND through every shell installed here the way sshd
        /// would (`<shell> -c <command>`), and checks each is recognised as
        /// itself. A Unix pwsh is declined like any Unix PowerShell, but what
        /// it prints shows the probe's words expand in PowerShell's argument
        /// mode — the same parser Windows PowerShell and a Windows pwsh use.
        #[cfg(unix)]
        #[test]
        fn real_shells_answer_the_probe_as_themselves() {
            use std::process::Command;
            let run = |bin: &str, args: &[&str]| {
                Command::new(bin)
                    .args(args)
                    .arg("-c")
                    .arg(PROBE_COMMAND)
                    .env("SHELL", format!("/usr/bin/{bin}"))
                    .output()
                    .ok()
            };
            for (bin, shell) in [
                ("sh", None),
                ("dash", None),
                ("bash", Some(RemoteShell::Bash)),
                ("zsh", Some(RemoteShell::Zsh)),
                ("fish", Some(RemoteShell::Fish)),
            ] {
                let Some(out) = run(bin, &[]) else { continue };
                let parsed = parse_probe(&String::from_utf8_lossy(&out.stdout));
                assert_eq!(parsed.map(|(s, _)| s), shell, "{bin}: {out:?}");
            }
            if let Some(out) = run("pwsh", &["-NoProfile", "-NonInteractive"]) {
                let text = String::from_utf8_lossy(&out.stdout);
                let lines: Vec<&str> = text.lines().collect();
                assert_eq!(lines.first(), Some(&PROBE_MARKER), "{text:?}");
                assert_eq!(lines.get(1), Some(&"__lyrel_psedition=Core"), "{text:?}");
                let home = lines
                    .get(2)
                    .and_then(|l| l.strip_prefix(PS_HOME_KEY))
                    .unwrap_or_default();
                assert!(
                    std::path::Path::new(home).join("pwsh").is_file(),
                    "{home:?} is not pwsh's home ({text:?})"
                );
                assert_eq!(parse_probe(&text), None);
                assert!(!probe_answered_by_cmd(&text));
            }
        }

        /// The script inside `-EncodedCommand`, and everything around it.
        fn split_powershell_bootstrap(script: &str) -> (&str, String, &str) {
            let (head, rest) = script
                .split_once("-EncodedCommand ")
                .expect("carries an encoded command");
            let (b64, tail) = rest.split_once(';').expect("ends with the exit");
            (head, super::super::tests::decode_utf16le_base64(b64), tail)
        }

        #[test]
        fn powershell_bootstrap_starts_the_same_powershell_with_the_integration() {
            let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None);
            assert!(
                !script.contains('\n'),
                "one line through the Win32 command line"
            );
            assert!(!script.contains('"'), "no argv quoting for sshd to eat");
            let (head, body, tail) = split_powershell_bootstrap(&script);
            assert_eq!(
                head,
                r"& 'C:\Program Files\PowerShell\7\pwsh.exe' -NoLogo -NoExit "
            );
            assert_eq!(tail, " exit $LASTEXITCODE");
            assert_eq!(body, compact_powershell(POWERSHELL_INTEGRATION));
            for needle in ["]133;A", "]133;B", "]133;C", "]133;D", "]7;file://", "]0;"] {
                assert!(body.contains(needle), "compacted away {needle}");
            }
            assert!(!body.lines().any(|l| l.starts_with('#')));

            let ps51 = bootstrap_command(RemoteShell::PowerShell, WIN_PS51, None);
            assert!(ps51.starts_with(&format!("& '{WIN_PS51}' -NoLogo -NoExit ")));
        }

        #[test]
        fn windows_start_dirs_are_taken_only_when_they_name_one_place() {
            let cases = [
                ("/C:/Users/ann/my dir", Some("C:/Users/ann/my dir")),
                ("C:/Users/ann", Some("C:/Users/ann")),
                (r"d:\work", Some(r"d:\work")),
                ("/C:/", Some("C:/")),
                ("//fs01/share/team", Some(r"\\fs01\share\team")),
                (r"\\fs01\share", Some(r"\\fs01\share")),
                // Relative to whichever drive is current.
                ("/Users/ann", None),
                // Wherever drive C last was.
                ("C:", None),
                ("/C:", None),
                ("C:Users", None),
                ("~/x", None),
                ("x", None),
                ("", None),
                ("//fs01", None),
                ("//fs01/", None),
                (r"\\?\C:\x", None),
                (r"\\.\pipe\x", None),
                ("C:/a\0b", None),
            ];
            for (dir, want) in cases {
                assert_eq!(powershell_start_dir(dir).as_deref(), want, "{dir:?}");
            }
        }

        #[test]
        fn a_start_dir_opens_the_encoded_script_with_one_quiet_set_location() {
            let dir = "/C:/Users/ann/it's ‘quoted’ $HOME";
            let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(dir));
            let (_, body, _) = split_powershell_bootstrap(&script);
            let (first, rest) = body.split_once('\n').unwrap();
            assert_eq!(
                first,
                "try { Set-Location -LiteralPath 'C:/Users/ann/it''s ‘‘quoted’’ $HOME' \
                 -ErrorAction Stop } catch { }"
            );
            assert_eq!(rest, compact_powershell(POWERSHELL_INTEGRATION));

            for refused in ["/Users/ann", "C:", "relative"] {
                assert_eq!(
                    bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(refused)),
                    bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None),
                    "{refused:?} must not become a Set-Location"
                );
            }
        }

        #[test]
        fn powershell_bootstrap_fits_a_windows_command_line() {
            // sshd's own part: the shell's path and its `-c`.
            let sshd = WIN_PWSH.len() + " -c ".len() + 2;
            let longest_classic = format!("C:/{}", "d".repeat(257));
            for dir in [None, Some(longest_classic.as_str())] {
                let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, dir);
                let len = script.chars().count();
                assert!(
                    len <= POWERSHELL_COMMAND_LINE_BUDGET && len + sshd < 32_767,
                    "{len} chars with start dir {dir:?}"
                );
            }
            // A long-path-aware host can report a far longer cwd. It costs the
            // start dir, not the session.
            let huge = format!("C:/{}", "d/".repeat(8_000));
            assert_eq!(
                bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(&huge)),
                bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None),
            );
        }

        #[test]
        fn ps_quote_doubles_every_single_quote_powershell_honours() {
            assert_eq!(ps_quote("a'b"), "'a''b'");
            assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
            assert_eq!(ps_quote(r"C:\x $y"), r"'C:\x $y'");
        }

        /// A throwaway home for a pwsh run, so no real profile is read.
        #[cfg(unix)]
        fn pwsh(home: &std::path::Path) -> std::process::Command {
            let mut cmd = std::process::Command::new("pwsh");
            cmd.current_dir(home)
                .env("HOME", home)
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("XDG_DATA_HOME", home.join(".local/share"))
                .env("XDG_CACHE_HOME", home.join(".cache"))
                .env_remove("LYREL_SHELL_INTEGRATION");
            cmd
        }

        /// Runs the bootstrap in a real pwsh the way a PowerShell default
        /// shell runs an exec request (`<shell> -c <bootstrap>`), with the
        /// Windows executable swapped for the local one and commands on stdin
        /// in place of a user, and checks the child decodes and runs the
        /// integration and hands its exit code back out through the parent.
        #[cfg(unix)]
        #[test]
        fn a_real_pwsh_runs_the_bootstrap_end_to_end() {
            use std::io::Write as _;
            use std::process::Stdio;

            let home = tempfile::tempdir().expect("tempdir");
            let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None).replacen(
                &ps_quote(WIN_PWSH),
                "'pwsh'",
                1,
            );
            let Ok(mut child) = pwsh(home.path())
                .args(["-NoProfile", "-c", &script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            else {
                return;
            };
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    b"Write-Output \"integrated=$env:LYREL_SHELL_INTEGRATION\"\n\
                      Write-Output \"prompt=$((Get-Command prompt).ScriptBlock -match '133;A')\"\n\
                      exit 7\n",
                )
                .unwrap();
            let out = child.wait_with_output().expect("pwsh ran");
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stdout.contains("integrated=1"), "{stdout}\n{stderr}");
            assert!(stdout.contains("prompt=True"), "{stdout}\n{stderr}");
            assert_eq!(out.status.code(), Some(7), "{stdout}\n{stderr}");
        }

        /// The start directory line in a real pwsh: it lands in a directory
        /// that is there, and says nothing and stays put for one that is gone.
        #[cfg(unix)]
        #[test]
        fn a_real_pwsh_lands_in_the_start_dir_and_stays_quiet_without_it() {
            let root = tempfile::tempdir().expect("tempdir");
            let root = root.path().canonicalize().expect("canonical tempdir");
            let home = root.join("home");
            let target = root.join("it's [a] ‘dir’");
            std::fs::create_dir_all(&home).expect("home");
            std::fs::create_dir_all(&target).expect("target");
            let missing = root.join("gone since");

            for (dir, want) in [(&target, &target), (&missing, &home)] {
                let script = format!(
                    "{}(Get-Location).Path",
                    set_location_line(dir.to_str().unwrap())
                );
                let Ok(out) = pwsh(&home)
                    .args(["-NoProfile", "-NonInteractive", "-c", &script])
                    .output()
                else {
                    return;
                };
                assert!(out.status.success(), "{out:?}");
                assert_eq!(
                    String::from_utf8_lossy(&out.stdout).trim_end(),
                    want.to_str().unwrap()
                );
                assert!(
                    out.stderr.is_empty(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_mode_detection_survives_rebound_escape_and_inputrc() {
        assert!(
            ZSH_INTEGRATION.contains("bindkey -lL main"),
            "zsh edit-mode detection must key off the main keymap link"
        );
        assert!(ZSH_INTEGRATION.contains("viins"));
    }

    #[test]
    fn is_our_zdotdir_matches_only_our_prefix() {
        assert!(is_our_zdotdir("/tmp/lyrel-zdotdir-1234-0"));
        assert!(is_our_zdotdir("lyrel-zdotdir-x"));
        assert!(!is_our_zdotdir("/home/alice/.config/zsh"));
        assert!(!is_our_zdotdir("/tmp/other-zdotdir"));
        assert!(!is_our_zdotdir(""));
        assert!(!is_our_zdotdir("/tmp/not-lyrel-zdotdir-1"));
    }

    /// `\e]133;D;1\a` — what a shell reports for the `false` these tests type.
    ///
    /// The trailing BEL is the point. `contains("133;D;1")` is a prefix match on
    /// the exit status, so it also accepts `133;D;127` (the bootstrap exec'd a
    /// shell that could not find `false`) and `133;D;130` (interrupted) — a
    /// bootstrap that never ran the command at all would pass every assertion
    /// below. Matching through the terminator pins the status whole.
    const FAILED_COMMAND_MARK: &str = "133;D;1;aid=lyrel\u{7}";

    /// Type `keys` at a freshly spawned shell and return everything it wrote back.
    ///
    /// `keys` carries its own Enter because the two families disagree about which
    /// byte that is. A line-discipline shell gets `\n` (the pty's ICRNL would
    /// accept `\r` too); PSReadLine puts the tty in raw mode and reads keys
    /// itself, so only a real `\r` submits — `\n` just sits in the buffer and the
    /// command never runs.
    fn prompt_cycle_over_pty(
        program: &str,
        injection: &Injection,
        keys: &[u8],
        cwd: Option<&Path>,
    ) -> String {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::io::{Read, Write};

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(program);
        cmd.args(&injection.args);
        for (k, v) in &injection.env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        let mut child = pty.slave.spawn_command(cmd).expect("spawn shell");
        // ConPTY's slave owns the same console as its master. Keeping it here
        // would prevent dropping the master from actually closing the console.
        drop(pty.slave);
        let bash_prompt = matches!(shell_kind(Some(program)), Some(ShellKind::Bash));

        let mut writer = pty.master.take_writer().expect("writer");
        let mut reader = pty.master.try_clone_reader().expect("reader");

        let (tx, rx) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut out = Vec::new();
        let mut answered = 0usize;
        let mut typed = false;
        let mut seen_fail = None;
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(chunk) => out.extend_from_slice(&chunk),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            let text = String::from_utf8_lossy(&out);

            // Play terminal for the one query that blocks: PSReadLine asks where
            // the cursor is (`CSI 6n`) before it will draw anything, and waits for
            // the answer. Nothing is behind this pty to give one, so an unanswered
            // report means pwsh never echoes a keystroke and the whole test times
            // out. The position itself does not matter here — only that a reply
            // arrives. A line-discipline shell never asks, so this is inert for
            // the Git Bash and WSL cases.
            let asked = text.matches("\u{1b}[6n").count();
            if asked > answered {
                for _ in answered..asked {
                    let _ = writer.write_all(b"\x1b[1;1R");
                }
                let _ = writer.flush();
                answered = asked;
            }

            // Type only once the first prompt is marked (A or B — they arrive
            // in the same prompt cycle for every integrated shell, and a
            // Nushell without a config-defined prompt_indicator emits only A).
            // Writing at spawn time instead puts the keystrokes ahead of the
            // cursor-position reply in the same input stream, and pwsh — still
            // waiting on that reply — eats `false\r` as the answer to its own
            // query. The command then never runs, and the failure reads as a
            // missing C mark rather than as the race it is.
            // Bash emits A from PROMPT_COMMAND before Readline owns the tty.
            // Wait for its displayed prompt so literal-next input is not
            // consumed by the tty's canonical line discipline instead.
            if !typed && (text.contains("133;B") || (!bash_prompt && text.contains("133;A"))) {
                writer.write_all(keys).expect("write");
                writer.flush().expect("flush");
                typed = true;
            }

            // Stop only once the typed command's own D report lands. A shell can
            // emit an unpaired D while drawing its first prompt, and breaking on
            // that would sample the transcript before the prompt cycle under test
            // has run at all.
            if typed && text.contains(FAILED_COMMAND_MARK) && seen_fail.is_none() {
                seen_fail = Some(std::time::Instant::now());
            }
            // The D mark and the rest of the same prompt cycle (the cwd report,
            // the A mark) are separate writes: breaking the moment the mark
            // arrives can sample the transcript before the report does. Hold
            // the pty open briefly so the cycle's tail lands.
            if let Some(at) = seen_fail {
                if at.elapsed() >= std::time::Duration::from_millis(500) {
                    break;
                }
            }
        }
        // Git's bin/bash.exe is a launcher: killing and waiting for that process
        // does not reap the actual Bash holding cwd open. Once the command has
        // finished, ask Bash itself to exit and let the launcher wait for it.
        // Keep teardown output out of `out`: it is not part of the sampled
        // consumer contract (in particular, it adds another command-start mark).
        let exit_status = if bash_prompt && seen_fail.is_some() {
            writer.write_all(b"builtin exit 0\n").expect("exit Bash");
            writer.flush().expect("flush Bash exit");
            Some(child.wait().expect("wait for Bash exit"))
        } else {
            // Reap before asserting even when the expected prompt never arrived.
            let _ = child.kill();
            child.wait().expect("wait for shell exit");
            None
        };
        drop(writer);
        drop(pty.master);
        reader_thread.join().expect("join PTY reader");
        if let Some(status) = exit_status {
            assert!(status.success(), "Bash did not exit cleanly: {status}");
        }
        assert!(
            typed,
            "no prompt-start mark ever arrived, so nothing was typed; got:\n{}",
            String::from_utf8_lossy(&out)
        );
        String::from_utf8_lossy(&out).into_owned()
    }

    fn reported_cwd(text: &str) -> PathBuf {
        let payload = text
            .split("\u{1b}]")
            .find(|s| s.starts_with("7;file://"))
            .and_then(|s| s.split(['\u{7}', '\u{1b}']).next())
            .unwrap_or_else(|| panic!("expected OSC 7; got:\n{text}"));
        crate::daemon::pane::facts::output_marks::parse_osc7(payload.as_bytes())
            .unwrap_or_else(|| panic!("daemon could not parse OSC 7 payload {payload:?}"))
    }

    /// The last OSC 7 cwd report in a transcript — the one emitted after the
    /// typed commands ran, which is what proves `cd` moved the pane's cwd.
    fn last_osc7(text: &str) -> PathBuf {
        let payload = text
            .split("\u{1b}]")
            .filter(|s| s.starts_with("7;file://"))
            .last()
            .and_then(|s| s.split(['\u{7}', '\u{1b}']).next())
            .unwrap_or_else(|| panic!("expected OSC 7; got:\n{text}"));
        crate::daemon::pane::facts::output_marks::parse_osc7(payload.as_bytes())
            .unwrap_or_else(|| panic!("daemon could not parse OSC 7 payload {payload:?}"))
    }

    #[cfg(windows)]
    #[test]
    fn git_bash_reports_the_full_prompt_cycle_over_a_real_pty() {
        let Some(bash) = crate::core::shells::git_bash_path() else {
            eprintln!("skipping: Git for Windows not installed");
            return;
        };
        let bash = bash.to_string_lossy().into_owned();
        let injection = setup(Some(&bash), &[], false).expect("bash integration");
        let text = prompt_cycle_over_pty(&bash, &injection, b"false\n", None);

        for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
            assert!(
                text.contains(mark),
                "Git Bash must report {mark:?}; got:\n{text}"
            );
        }
        let cwd = reported_cwd(&text);
        assert!(
            cwd.exists(),
            "Git Bash reported a cwd the Windows side cannot resolve: {cwd:?} \
             — a drive-relative msys path, so `pwd -W` translation regressed"
        );
    }

    /// Exercise the installed integration in an actual interactive Bash, not
    /// by invoking its private functions. LYREL_TEST_BASH permits the same
    /// consumer contract to run against an installed Bash 3.2 or 4.4 as well.
    fn bash_contract_over_pty(array_prompt: bool, keys: &[u8], inputrc: &str) -> Option<String> {
        let program = std::env::var("LYREL_TEST_BASH").ok().or_else(|| {
            #[cfg(windows)]
            {
                crate::core::shells::git_bash_path().map(|p| p.to_string_lossy().into_owned())
            }
            #[cfg(not(windows))]
            {
                Path::new("/bin/bash")
                    .is_file()
                    .then(|| "/bin/bash".to_string())
            }
        });
        let Some(program) = program else {
            eprintln!("skipping: no Bash installed");
            return None;
        };
        let home = throwaway_dir("lyrel-bash-contract-").expect("fixture home");
        let cwd = home.join("cwd%20literal");
        let cleanup = home.join("remote-rc");
        std::fs::create_dir(&cwd).expect("fixture cwd");
        std::fs::create_dir(&cleanup).expect("remote bootstrap directory");
        std::fs::write(home.join("inputrc"), inputrc).expect("inputrc");
        let fixture = r#"
# The direct usr/bin executable does not run Git's bin/bash.exe PATH wrapper.
if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]]; then
  PATH="/usr/bin:/bin:$PATH"
fi
HISTCONTROL=ignoredups
HISTIGNORE=not-a-command
shopt -s cmdhist lithist
__consumer_prompt() {
  local ret=$? pipes="${PIPESTATUS[*]}" last=$_
  builtin printf '\e]777;prompt;%s;%s;%s;%s;%s\a' "$ret" "$pipes" "$last" "$HISTCONTROL" "$HISTIGNORE"
  PS1='consumer> '
  return "$ret"
}
trap 'builtin printf "\e]777;debug;%s;%s;%s\a" "$?" "${PIPESTATUS[*]}" "$_"' DEBUG
PS0='$(builtin printf "\e]777;user-ps0\a")'
if [[ "$LYREL_TEST_ARRAY_PROMPT" == 1 ]] &&
    (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
  PROMPT_COMMAND=([2]='__consumer_prompt' [7]=$'builtin printf "\e]777;array-tail\a"\nPS1="array> "')
else
  PROMPT_COMMAND=$'__consumer_prompt\n# a trailing comment must not swallow the integration'
fi
builtin printf '\e]777;version;%s.%s\a' "${BASH_VERSINFO[0]}" "${BASH_VERSINFO[1]}"
"#;
        let rcfile = home.join("bashrc");
        std::fs::write(
            &rcfile,
            format!("{fixture}{BASH_INTEGRATION}{}", remote::BASH_CLEANUP_HOOK),
        )
        .expect("fixture rcfile");
        let injection = Injection {
            env: HashMap::from([
                ("HOME".into(), bash_path(&home)),
                ("HISTFILE".into(), bash_path(&home.join("history"))),
                ("INPUTRC".into(), bash_path(&home.join("inputrc"))),
                ("LYREL_SHELL_INTEGRATION".into(), String::new()),
                (
                    "LYREL_TEST_ARRAY_PROMPT".into(),
                    u8::from(array_prompt).to_string(),
                ),
                ("LYREL_RM_DIR".into(), bash_path(&cleanup)),
            ]),
            args: vec![
                "--noprofile".into(),
                "--rcfile".into(),
                bash_path(&rcfile),
                "-i".into(),
            ],
            replaces_argv: true,
            dir: Some(home.clone()),
        };
        let text = prompt_cycle_over_pty(&program, &injection, keys, Some(&cwd));
        assert!(
            !cleanup.exists(),
            "SSH rc directory was not cleaned up:\n{text}"
        );
        let reported = reported_cwd(&text);
        assert!(reported.exists(), "unusable cwd {reported:?}:\n{text}");
        assert_eq!(reported.file_name().unwrap(), "cwd%20literal");
        std::fs::remove_dir_all(home).expect("remove fixture home");
        Some(text)
    }

    fn osc_payloads(text: &str) -> Vec<&str> {
        text.split("\u{1b}]")
            .skip(1)
            .filter_map(|part| part.split_once('\u{7}').map(|(payload, _)| payload))
            .collect()
    }

    #[test]
    fn bash_preserves_native_prompt_and_trap_contracts_over_a_real_pty() {
        let keys = b"\n: LAST_ARGUMENT\nfalse | (exit 7)\nbuiltin printf '\\033]777;state;%s;%s;%s\\a' \"$?\" \"${PIPESTATUS[*]}\" \"$_\"\nprintf '%s\\n' 'multi\nline'\nfalse | true\nfalse\n";
        for array_prompt in [false, true] {
            let Some(text) = bash_contract_over_pty(array_prompt, keys, "set editing-mode vi\n")
            else {
                return;
            };
            let payloads = osc_payloads(&text);
            let version = payloads
                .iter()
                .find_map(|p| p.strip_prefix("777;version;"))
                .expect("Bash version");
            let mut version = version.split('.').map(|n| n.parse::<u32>().unwrap());
            let version = (version.next().unwrap(), version.next().unwrap());
            let commands: Vec<_> = payloads
                .iter()
                .copied()
                .filter(|p| p.starts_with("133;C;"))
                .collect();
            assert_eq!(
                commands.len(),
                6,
                "empty input/pipelines must not add C marks:\n{text}"
            );
            assert_eq!(commands[0], "133;C;: LAST_ARGUMENT;aid=lyrel", "{text}");
            assert_eq!(commands[1], "133;C;false | (exit 7);aid=lyrel", "{text}");
            assert_eq!(
                commands[3], "133;C;printf '%25s\\n' 'multi%0Aline';aid=lyrel",
                "{text}"
            );
            assert_eq!(commands[4], "133;C;false | true;aid=lyrel", "{text}");
            let finished: Vec<_> = payloads
                .iter()
                .copied()
                .filter(|p| p.starts_with("133;D;"))
                .collect();
            assert_eq!(
                finished,
                [
                    "133;D;0;aid=lyrel",
                    "133;D;7;aid=lyrel",
                    "133;D;0;aid=lyrel",
                    "133;D;0;aid=lyrel",
                    "133;D;0;aid=lyrel",
                    "133;D;1;aid=lyrel"
                ],
                "{text}"
            );
            assert!(
                payloads
                    .iter()
                    .filter(|p| p.starts_with("777;prompt;"))
                    .all(|p| p.ends_with(";ignoredups;not-a-command")),
                "prompt changed the user's history settings:\n{text}"
            );
            assert!(
                payloads.iter().any(|p| p.starts_with("777;prompt;7;")),
                "prompt lost the command's exit status:\n{text}"
            );
            // The frozen Bash 3.2 baseline also loses prompt $_/PIPESTATUS.
            // Native PS0 must preserve them; do not pin legacy helper values.
            if version >= (4, 4) {
                assert!(
                    payloads.contains(&"777;prompt;0;0;LAST_ARGUMENT;ignoredups;not-a-command"),
                    "prompt lost $_:\n{text}"
                );
                assert!(
                    payloads.iter().any(|p| p.starts_with("777;prompt;7;1 7;")),
                    "prompt lost pipeline status:\n{text}"
                );
            }
            assert!(
                payloads.iter().any(|p| p.starts_with("777;debug;7;1 7;")),
                "user DEBUG trap was lost or received modified state:\n{text}"
            );
            // Bash preserves $? across PROMPT_COMMAND, but the user's own
            // prompt commands may replace PIPESTATUS after capturing it.
            assert!(
                payloads.iter().any(|p| p.starts_with("777;state;7;")),
                "next command lost exit status:\n{text}"
            );
            assert!(
                payloads.contains(&"133;V;1"),
                "inputrc vi mode was ignored:\n{text}"
            );
            for (index, payload) in payloads.iter().enumerate() {
                if !payload.starts_with("133;D;") {
                    continue;
                }
                let tail = &payloads[index + 1..];
                let prompt = tail
                    .iter()
                    .position(|p| p.starts_with("777;prompt;"))
                    .expect("user prompt");
                let a = tail.iter().position(|p| *p == "133;A").expect("A after D");
                let b = tail.iter().position(|p| *p == "133;B").expect("B after D");
                assert!(
                    prompt < a && a < b,
                    "D/user hooks/A/B order changed:\n{text}"
                );
            }
            let ps0_count = payloads.iter().filter(|p| **p == "777;user-ps0").count();
            assert_eq!(
                ps0_count,
                if version >= (4, 4) { 6 } else { 0 },
                "user PS0 changed:\n{text}"
            );
            let prompt = if array_prompt && version >= (5, 1) {
                "array> "
            } else {
                "consumer> "
            };
            assert!(
                text.contains(&format!("{prompt}\u{1b}]133;B\u{7}")),
                "framework PS1 lost its final B:\n{text}"
            );
        }
    }

    #[test]
    fn bash_escapes_and_truncates_submitted_commands_over_a_real_pty() {
        // Use Readline's public macro, retaining ESC/BEL and a multiline command.
        // Git Bash's parser drops literal CR even with `bash -c`; Unix can
        // submit it verbatim, so only that fixture includes the extra character.
        let (insert_cr, encoded_cr, remaining) = if cfg!(windows) {
            ("", "", 483)
        } else {
            ("\\C-v\\C-m", "%0D", 482)
        };
        let inputrc = format!(
            "set editing-mode vi\nset keymap vi-insert\n\
             \"\\C-x\\C-u\": \"printf '%s' 'percent%\\C-v\\e\\C-v\\C-g{insert_cr}multi\\n{}'\\nfalse\\n\"\n",
            "x".repeat(600)
        );
        let Some(text) = bash_contract_over_pty(false, b"\x18\x15", &inputrc) else {
            return;
        };
        let payloads = osc_payloads(&text);
        let commands: Vec<_> = payloads
            .iter()
            .copied()
            .filter(|p| p.starts_with("133;C;"))
            .collect();
        // The literal prefix consumes 30 characters (29 without CR), leaving
        // exactly 482 or 483 x's within the 512-character protocol limit.
        let escaped = format!(
            "133;C;printf '%25s' 'percent%25%1B%07{encoded_cr}multi%0A{};aid=lyrel",
            "x".repeat(remaining)
        );
        assert_eq!(
            commands,
            [escaped, "133;C;false;aid=lyrel".to_string()],
            "{text}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn wsl_reports_the_full_prompt_cycle_over_a_real_pty() {
        let Some(distro) = crate::core::shells::wsl_distros().into_iter().next() else {
            eprintln!("skipping: no WSL distributions installed");
            return;
        };
        let args: Vec<String> = vec![
            "--distribution".into(),
            distro.clone(),
            "--cd".into(),
            "~".into(),
        ];
        let injection = setup(Some("wsl.exe"), &args, false).expect("wsl integration");
        let text = prompt_cycle_over_pty("wsl.exe", &injection, b"false\n", None);

        for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
            assert!(
                text.contains(mark),
                "WSL ({distro}) must report {mark:?}; got:\n{text}"
            );
        }
        let cwd = reported_cwd(&text);
        assert!(
            cwd.to_string_lossy().starts_with('/'),
            "expected the distro's own absolute path, got {cwd:?}"
        );
    }

    /// The OSC 0 title the shell settled on, as `user@host:dir`.
    #[cfg(unix)]
    fn reported_title(text: &str) -> String {
        text.split("\u{1b}]")
            .filter_map(|s| s.strip_prefix("0;"))
            .filter_map(|s| s.split(['\u{7}', '\u{1b}']).next())
            // pwsh emits an empty OSC 0 of its own just before ours; the one
            // under test is whatever is left after dropping those.
            .filter(|t| !t.is_empty())
            .last()
            .unwrap_or_else(|| panic!("expected a non-empty OSC 0 title; got:\n{text}"))
            .to_string()
    }

    #[cfg(unix)]
    fn pwsh_on_path() -> Option<String> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("pwsh"))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Guards the Unix half of the PowerShell integration, which had no coverage
    /// at all until #583 — every pty round-trip here used to be `#[cfg(windows)]`,
    /// so a script written against `$env:USERNAME` / `$env:COMPUTERNAME` /
    /// `$env:USERPROFILE` shipped for two platforms where all three are empty.
    #[cfg(unix)]
    #[test]
    fn pwsh_reports_the_full_prompt_cycle_over_a_real_pty_on_unix() {
        let Some(pwsh) = pwsh_on_path() else {
            eprintln!("skipping: pwsh not installed");
            return;
        };
        let injection = setup(Some(&pwsh), &[], false).expect("powershell integration");
        // From $HOME, so the title's `~` abbreviation is exercised rather than
        // assumed — it is the half that silently did nothing on Unix.
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let text = prompt_cycle_over_pty(&pwsh, &injection, b"false\r", Some(&home));

        for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
            assert!(
                text.contains(mark),
                "pwsh must report {mark:?}; got:\n{text}"
            );
        }

        let cwd = reported_cwd(&text);
        assert_eq!(
            cwd,
            home.canonicalize().unwrap_or(home.clone()),
            "OSC 7 must name the pane's real cwd"
        );

        // Asserted by shape, not against $USER: a CI runner does not reliably
        // export it, and the bug being guarded is emptiness — `@:` — not which
        // name lands there. When the environment does name the user, pin it.
        let title = reported_title(&text);
        let (who, path) = title.split_once(':').expect("title is `user@host:dir`");
        let (user, host) = who.split_once('@').expect("title is `user@host:dir`");
        assert!(
            !user.is_empty(),
            "the title must name the user — `$env:USERNAME` is empty on Unix, \
             which is what left it starting with a bare `@`; got {title:?}"
        );
        assert!(
            !host.is_empty(),
            "the title must name the host — `$env:COMPUTERNAME` is empty on Unix; \
             got {title:?}"
        );
        if let Ok(expected) = std::env::var("USER") {
            assert_eq!(user, expected, "the title named the wrong user");
        }
        assert_eq!(
            path, "~",
            "$HOME must abbreviate to `~` — the old check keyed off \
             `$env:USERPROFILE`, which does not exist on Unix; got {title:?}"
        );
    }

    #[test]
    fn shell_kind_maps_known_basenames() {
        assert!(matches!(shell_kind(Some("/bin/zsh")), Some(ShellKind::Zsh)));
        assert!(matches!(shell_kind(Some("zsh")), Some(ShellKind::Zsh)));
        assert!(matches!(
            shell_kind(Some("/bin/bash")),
            Some(ShellKind::Bash)
        ));
        assert!(matches!(
            shell_kind(Some("/usr/local/bin/fish")),
            Some(ShellKind::Fish)
        ));
        for prog in [
            "powershell.exe",
            "powershell",
            "pwsh",
            "pwsh.exe",
            "C:/Program Files/PowerShell/7/pwsh.exe",
            "PowerShell.EXE",
        ] {
            assert!(
                matches!(shell_kind(Some(prog)), Some(ShellKind::PowerShell)),
                "{prog} should map to PowerShell"
            );
        }
        for prog in ["nu", "nu.exe", "C:/Tools/nu.exe"] {
            assert!(
                matches!(shell_kind(Some(prog)), Some(ShellKind::Nushell)),
                "{prog} should map to Nushell"
            );
        }
        assert!(shell_kind(Some("/bin/sh")).is_none());
        assert!(shell_kind(Some("cmd.exe")).is_none());
        assert!(matches!(shell_kind(Some("wsl.exe")), Some(ShellKind::Wsl)));
        assert!(matches!(shell_kind(Some("wsl")), Some(ShellKind::Wsl)));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_setup_never_contacts_the_distro() {
        let args: Vec<String> = vec![
            "--distribution".into(),
            "lyrel-no-such-distro-exists".into(),
            "--cd".into(),
            "~".into(),
        ];
        let inj = setup(Some("wsl.exe"), &args, false)
            .expect("setup must not depend on reaching the distro");

        let sep = inj
            .args
            .iter()
            .position(|a| a == "--exec")
            .expect("`--exec`");
        assert_eq!(
            &inj.args[..sep],
            &[
                "--distribution".to_string(),
                "lyrel-no-such-distro-exists".to_string(),
                "--cd".to_string(),
                "~".to_string()
            ]
        );
        assert_eq!(inj.args[sep + 1], "sh");
        assert_eq!(inj.args[sep + 2], "-c");
        assert!(inj.args[sep + 3].contains("$SHELL"));
        assert!(inj.args[sep + 3].contains("--rcfile"));
        assert!(inj.replaces_argv);
    }

    #[test]
    fn the_wsl_bootstrap_carries_integration_for_fish_not_just_bash() {
        let script = wsl_exec_script();

        assert!(script.contains(
            r#"*/bash) [ -r "${LYREL_RC:-}" ] && exec "$SHELL" --rcfile "$LYREL_RC" -i; exec "$SHELL" -l ;;"#
        ));
        assert!(script.contains(&format!(
            r#"*/fish) exec "$SHELL" -C {} -l ;;"#,
            shell_quote(FISH_INTEGRATION)
        )));

        // `sh` parses this script, so the body is POSIX-quoted — backslashes
        // pass through untouched. `remote::fish_quote`, which the SSH path uses
        // because the far end's own fish parses the bootstrap there, would
        // double every one of them and hand fish `\\e]%s\\a`.
        assert!(script.contains(r"printf '\''\e]%s\a'\'' $argv[1]"));
    }

    #[test]
    fn the_wsl_bootstrap_carries_integration_for_zsh_too() {
        let script = wsl_exec_script();

        assert!(
            script.contains(
                r#"*/zsh) [ -n "${LYREL_ZDOTDIR:-}" ] && [ -r "$LYREL_ZDOTDIR/.zshrc" ] "#
            )
        );
        assert!(script.contains(r#"&& export ZDOTDIR="$LYREL_ZDOTDIR"; exec "$SHELL" -l ;;"#));

        // The redirectors read this to find the user's own startup files, and
        // only the distro can answer it. Naming it out here would aim them at a
        // Windows path no distro has.
        assert!(
            !script.contains("LYREL_USER_ZDOTDIR"),
            "the bootstrap must not carry a ZDOTDIR this side made up"
        );
    }

    #[test]
    fn wsl_integration_env_writes_what_each_shell_reads_and_names_it_for_the_distro() {
        let dir = throwaway_dir("lyrel-wsltest-").expect("temp dir");
        let env = wsl_integration_env(&dir, None).expect("integration files");

        let rcfile = PathBuf::from(env.get(WSL_RCFILE_ENV).expect("LYREL_RC"));
        assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), bash_rcfile());

        let zdotdir = PathBuf::from(env.get(WSL_ZDOTDIR_ENV).expect("LYREL_ZDOTDIR"));
        for (name, contents) in zsh_redirectors() {
            assert_eq!(
                std::fs::read_to_string(zdotdir.join(name)).unwrap(),
                contents
            );
        }

        // Both are Windows paths — `/p` is what turns them into the distro's
        // view of themselves, so neither may be pre-translated here.
        assert_eq!(env.get("WSLENV").unwrap(), "LYREL_RC/p:LYREL_ZDOTDIR/p");
        assert!(!env.contains_key("LYREL_USER_ZDOTDIR"));
        assert!(!env.contains_key("ZDOTDIR"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The arms are strings until something runs them. This asks a real `sh`
    /// which one it picks, with stub shells standing in for the distro's.
    #[cfg(unix)]
    #[test]
    fn the_wsl_bootstrap_picks_the_arm_that_matches_the_distro_s_shell() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;

        let dir = throwaway_dir("lyrel-wslarm-").expect("temp dir");
        for name in ["bash", "zsh", "fish"] {
            let stub = dir.join(name);
            std::fs::write(
                &stub,
                format!("#!/bin/sh\necho {name} \"$@\"\necho ZDOTDIR=${{ZDOTDIR-unset}}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let zdotdir = dir.join("zdot");
        std::fs::create_dir_all(&zdotdir).unwrap();
        std::fs::write(zdotdir.join(".zshrc"), "").unwrap();

        let rcfile = dir.join("bashrc");
        std::fs::write(&rcfile, "").unwrap();

        let run = |shell: &str, rc: &str, zdot: &str| {
            let out = Command::new("sh")
                .arg("-c")
                .arg(wsl_exec_script())
                .env_clear()
                .env("SHELL", dir.join(shell).to_string_lossy().into_owned())
                .env("LYREL_RC", rc)
                .env("LYREL_ZDOTDIR", zdot)
                .output()
                .expect("run the bootstrap");
            assert!(out.status.success(), "the bootstrap exited non-zero");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };

        let rc = rcfile.to_string_lossy().into_owned();
        let zdot = zdotdir.to_string_lossy().into_owned();
        assert!(run("bash", &rc, &zdot).contains("bash --rcfile"));
        assert!(run("fish", &rc, &zdot).starts_with("fish -C"));

        let zsh = run("zsh", &rc, &zdot);
        assert!(zsh.contains("zsh -l"), "{zsh}");
        assert!(zsh.contains(&format!("ZDOTDIR={zdot}")), "{zsh}");

        // The distro that cannot see /mnt. Every arm that was handed a path has
        // to notice, and fall back to the login shell that reads the user's own
        // files rather than to one holding a path that is not there.
        let missing = dir.join("not-there").to_string_lossy().into_owned();

        let blind_zsh = run("zsh", &rc, &missing);
        assert!(blind_zsh.contains("zsh -l"), "{blind_zsh}");
        assert!(blind_zsh.contains("ZDOTDIR=unset"), "{blind_zsh}");

        let blind_bash = run("bash", &missing, &zdot);
        assert!(
            !blind_bash.contains("--rcfile"),
            "bash was handed an rcfile it cannot read: {blind_bash}"
        );
        assert!(blind_bash.contains("bash -l"), "{blind_bash}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The distro runs this through `sh`, so a quoting slip is a pane that
    /// never opens. CI's Linux and macOS legs have a real `sh` to ask.
    #[cfg(unix)]
    #[test]
    fn the_wsl_bootstrap_is_valid_posix_sh() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let mut child = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh -n");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(wsl_exec_script().as_bytes())
            .expect("write script");
        let out = child.wait_with_output().expect("wait for sh -n");
        assert!(
            out.status.success(),
            "sh rejected the WSL bootstrap:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn wsl_distro_and_cd_are_read_from_either_flag_spelling() {
        let long: Vec<String> = ["--distribution", "Ubuntu-24.04", "--cd", "~"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wsl_distro(&long).as_deref(), Some("Ubuntu-24.04"));
        assert_eq!(wsl_cd(&long).as_deref(), Some("~"));

        let short: Vec<String> = ["-d", "Debian"].iter().map(|s| s.to_string()).collect();
        assert_eq!(wsl_distro(&short).as_deref(), Some("Debian"));
        assert_eq!(wsl_cd(&short), None);

        let eq: Vec<String> = ["--distribution=Arch", "--cd=/tmp"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wsl_distro(&eq).as_deref(), Some("Arch"));
        assert_eq!(wsl_cd(&eq).as_deref(), Some("/tmp"));

        assert_eq!(wsl_distro(&[]), None);
        assert_eq!(wsl_distro(&["--distribution".to_string()]), None);
    }

    #[test]
    fn wslenv_preserves_the_users_own_entries() {
        assert_eq!(
            wslenv_with(Some("MYVAR/p:OTHER"), &["LYREL_RC/p"]),
            "MYVAR/p:OTHER:LYREL_RC/p"
        );
        assert_eq!(wslenv_with(None, &["LYREL_RC/p"]), "LYREL_RC/p");
        assert_eq!(wslenv_with(Some(""), &["LYREL_RC/p"]), "LYREL_RC/p");
        assert_eq!(
            wslenv_with(Some("LYREL_RC/l"), &["LYREL_RC/p"]),
            "LYREL_RC/l"
        );
        assert_eq!(
            wslenv_with(None, &["LYREL_RC/p", "LYREL_ZDOTDIR/p"]),
            "LYREL_RC/p:LYREL_ZDOTDIR/p"
        );
        assert_eq!(
            wslenv_with(Some("LYREL_ZDOTDIR/l"), &["LYREL_RC/p", "LYREL_ZDOTDIR/p"]),
            "LYREL_ZDOTDIR/l:LYREL_RC/p"
        );
    }

    #[test]
    fn shell_kind_strips_exe_for_non_powershell_shells() {
        for prog in [
            "C:/Program Files/Git/bin/bash.exe",
            "C:/msys64/usr/bin/bash.exe",
        ] {
            assert!(
                matches!(shell_kind(Some(prog)), Some(ShellKind::Bash)),
                "{prog} should map to Bash"
            );
        }
        if !cfg!(windows) {
            for prog in ["bash.exe", "BASH.EXE", "bash"] {
                assert!(matches!(shell_kind(Some(prog)), Some(ShellKind::Bash)));
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn shell_kind_declines_the_wsl_bash_launcher() {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for prog in [
            format!(r"{system_root}\System32\bash.exe"),
            format!(r"{system_root}/System32/bash.exe"),
            format!(r"{system_root}\SysWOW64\bash.exe"),
            format!(r"{system_root}\system32\BASH.EXE"),
        ] {
            assert!(
                shell_kind(Some(&prog)).is_none(),
                "{prog} is the WSL launcher and must not be treated as Bash"
            );
        }
        for prog in ["bash", "bash.exe", "BASH.EXE"] {
            assert!(
                shell_kind(Some(prog)).is_none(),
                "{prog} cannot be identified as msys bash and must be declined"
            );
        }
    }

    #[test]
    fn bash_rcfile_path_uses_forward_slashes_on_windows() {
        let rendered = bash_path(Path::new(
            r"C:\Users\a\AppData\Local\Temp\lyrel-bashrc-1-0\bashrc",
        ));
        if cfg!(windows) {
            assert_eq!(
                rendered,
                "C:/Users/a/AppData/Local/Temp/lyrel-bashrc-1-0/bashrc"
            );
        }
        assert_eq!(
            bash_path(Path::new("/tmp/lyrel-bashrc-1-0/bashrc")),
            "/tmp/lyrel-bashrc-1-0/bashrc"
        );
    }

    #[test]
    fn zsh_redirectors_source_user_files_and_append_integration() {
        let files = zsh_redirectors();
        assert_eq!(files.len(), 4);
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, [".zshenv", ".zprofile", ".zshrc", ".zlogin"]);
        for (name, body) in &files {
            assert!(
                body.contains("$LYREL_USER_ZDOTDIR"),
                "{name} should reference the user's real ZDOTDIR"
            );
            assert!(body.contains(name), "{name} should source its own name");
            assert!(body.contains("source"), "{name} should source");
        }
        let zshrc = &files[2].1;
        assert!(zshrc.contains("__lyrel_precmd"));
        assert!(zshrc.contains("133;A"));
        assert!(!files[0].1.contains("__lyrel_precmd"));
    }

    #[test]
    fn zsh_redirectors_point_zdotdir_at_the_real_dir_only_while_sourcing() {
        for (name, body) in zsh_redirectors() {
            let save = body.find("__lyrel_ztmp=$ZDOTDIR").expect("stashes our dir");
            let aim = body
                .find("ZDOTDIR=$LYREL_USER_ZDOTDIR")
                .expect("aims at the real dir");
            let source = body
                .find(&format!("source \"${{ZDOTDIR:-$HOME}}/{name}\""))
                .expect("sources the user's file");
            let restore = body
                .rfind("ZDOTDIR=$__lyrel_ztmp")
                .expect("restores our dir");
            let unset = body
                .rfind("unset __lyrel_ztmp")
                .expect("cleans up its scratch var");
            assert!(
                save < aim && aim < source && source < restore && restore < unset,
                "{name}: order must be stash → aim-at-real → source → restore-ours → unset"
            );
        }
    }

    #[test]
    fn zshenv_recaptures_a_user_relocated_zdotdir() {
        let files = zsh_redirectors();
        let zshenv = &files[0].1;
        let source = zshenv.find("source \"${ZDOTDIR:-$HOME}/.zshenv\"").unwrap();
        let recapture = zshenv
            .find("export LYREL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}")
            .unwrap();
        let restore = zshenv.rfind("ZDOTDIR=$__lyrel_ztmp").unwrap();
        assert!(
            source < recapture && recapture < restore,
            "recapture must run after sourcing the user's .zshenv, before we restore our dir"
        );
        for (name, body) in &files[1..] {
            assert!(
                !body.contains("export LYREL_USER_ZDOTDIR"),
                "{name} must not re-export LYREL_USER_ZDOTDIR"
            );
        }
    }

    #[test]
    fn zsh_integration_restores_real_zdotdir_after_startup() {
        assert!(ZSH_INTEGRATION.contains("__lyrel_restore_zdotdir"));
        assert!(ZSH_INTEGRATION.contains("ZDOTDIR=${LYREL_USER_ZDOTDIR:-$HOME}"));
        assert!(
            ZSH_INTEGRATION.contains("add-zsh-hook -d precmd __lyrel_restore_zdotdir"),
            "the restore hook must deregister itself so it runs exactly once"
        );
    }

    #[test]
    fn bash_rcfile_sources_user_config_then_appends_integration() {
        let rc = bash_rcfile();
        assert!(rc.contains("/etc/profile"));
        assert!(rc.contains("~/.bash_profile"));
        assert!(rc.contains("~/.bashrc"));
        assert!(rc.contains("__lyrel"));
        assert!(rc.contains("133;"));
    }

    /// Runs `bash_rcfile()` against a throwaway $HOME whose ~/.bashrc appends a
    /// line every time it is sourced, and reports how many lines it left.
    ///
    /// `None` means there is no usable bash on this box, so there is nothing to
    /// assert either way.
    #[cfg(unix)]
    fn bashrc_sourcings(profile: Option<(&str, &str)>) -> Option<usize> {
        let home = throwaway_dir("lyrel-rcfile-home-")?;
        let ticks = home.join("ticks");
        if let Some((name, body)) = profile {
            std::fs::write(home.join(name), body).expect("write profile");
        }
        std::fs::write(
            home.join(".bashrc"),
            format!("printf 'tick\\n' >> '{}'\n", ticks.display()),
        )
        .expect("write .bashrc");
        let rcfile = home.join("rcfile");
        std::fs::write(&rcfile, bash_rcfile()).expect("write rcfile");

        let ran = std::process::Command::new("bash")
            .arg("--rcfile")
            .arg(&rcfile)
            .args(["-i", "-c", "true"])
            .env("HOME", &home)
            .output();
        let sourced = ran.ok().map(|_| {
            std::fs::read_to_string(&ticks)
                .unwrap_or_default()
                .lines()
                .count()
        });
        let _ = std::fs::remove_dir_all(&home);
        sourced
    }

    /// The profile chain is first-match-wins, and a ~/.bash_profile that exists
    /// only to forward to ~/.bashrc is the common case — so sourcing ~/.bashrc
    /// after the chain ran it a second time for almost everybody.
    #[cfg(unix)]
    #[test]
    fn a_forwarding_bash_profile_does_not_pull_bashrc_in_twice() {
        for forward in [
            "if [ -f ~/.bashrc ]; then . ~/.bashrc; fi\n",
            "source \"$HOME/.bashrc\"\n",
        ] {
            let Some(sourced) = bashrc_sourcings(Some((".bash_profile", forward))) else {
                return;
            };
            assert_eq!(
                sourced, 1,
                "~/.bashrc must be sourced exactly once for {forward:?}, got {sourced}",
            );
        }
    }

    /// The other half of the rule: a profile that never forwards is why the
    /// unconditional source existed in the first place. Dropping it outright
    /// would leave this user with no aliases, prompt or completions at all.
    #[cfg(unix)]
    #[test]
    fn a_profile_that_never_forwards_still_gets_bashrc_once() {
        for profile in [
            Some((".bash_profile", "export LYREL_PROFILE=1\n")),
            Some((".profile", "export LYREL_PROFILE=1\n")),
            // No profile at all — the plain macOS $HOME.
            None,
        ] {
            let Some(sourced) = bashrc_sourcings(profile) else {
                return;
            };
            assert_eq!(
                sourced, 1,
                "~/.bashrc must still arrive for {profile:?}, got {sourced}",
            );
        }
    }

    #[test]
    fn every_integration_guards_install_on_empty_sentinel() {
        for (shell, body) in [("zsh", ZSH_INTEGRATION), ("fish", FISH_INTEGRATION)] {
            assert!(
                body.contains(r#"-z "$LYREL_SHELL_INTEGRATION""#),
                "{shell} integration must guard install on the sentinel being empty \
                 (matching setup()'s empty-string reset), not on its mere definedness",
            );
        }
        assert!(
            !FISH_INTEGRATION.contains("set -q LYREL_SHELL_INTEGRATION"),
            "fish must guard on emptiness (`test -z`), never `set -q`",
        );
    }

    #[test]
    fn d_emitter_is_prepended_ahead_of_user_precmd_hooks() {
        assert!(
            ZSH_INTEGRATION.contains("precmd_functions=(__lyrel_precmd_d $precmd_functions)"),
            "zsh must prepend the D emitter (add-zsh-hook can only append)"
        );
        for (shell, body) in [("zsh", ZSH_INTEGRATION), ("fish", FISH_INTEGRATION)] {
            assert_eq!(
                body.matches("133;D").count(),
                1,
                "{shell} must emit D from exactly one place"
            );
        }
    }

    #[test]
    fn every_cwd_report_escapes_literal_percent() {
        for (shell, body, escape) in [
            ("zsh", ZSH_INTEGRATION, r"${PWD//\%/%25}"),
            (
                "fish",
                FISH_INTEGRATION,
                "string replace --all '%' '%25' -- $PWD",
            ),
        ] {
            assert!(
                body.contains(escape),
                "{shell}'s OSC 7 reporter must %-escape the literal percent"
            );
            assert!(
                !body.contains(r#" "$PWD";"#),
                "{shell} must not emit the raw $PWD in its OSC 7 report"
            );
        }
    }

    #[test]
    fn msys_payload_round_trips_through_parse_osc7() {
        let parse = |payload: &str| {
            crate::daemon::pane::facts::output_marks::parse_osc7(payload.as_bytes())
                .unwrap_or_else(|| panic!("{payload} should parse"))
        };
        for (translated, want) in [
            ("C:/Users/thoma/repo", "C:/Users/thoma/repo"),
            ("C:/", "C:/"),
            ("D:/work/a b", "D:/work/a b"),
            ("C:/tmp/a%25c", "C:/tmp/a%c"),
        ] {
            let got = parse(&format!("7;file://localhost/{translated}"));
            let want = if cfg!(windows) {
                PathBuf::from(want)
            } else {
                PathBuf::from(format!("/{want}"))
            };
            assert_eq!(got, want, "payload for {translated}");
        }

        if cfg!(windows) {
            let got = parse("7;file://localhost/c/Users/thoma");
            assert_ne!(got, PathBuf::from("C:/Users/thoma"));
            assert!(
                !got.is_absolute(),
                "{got:?} is drive-relative — Windows resolves it against the \
                 current drive, which is why it must never be emitted"
            );
        }
    }

    #[test]
    fn setup_fish_injects_startup_command_without_files() {
        let inj = setup_fish().expect("fish injection is infallible");
        assert_eq!(inj.args[0], "-C");
        assert!(inj.args[1].contains("__lyrel"));
        assert!(inj.args[1].contains("133;"));
        assert!(inj.env.is_empty());
        assert!(!inj.replaces_argv);
        assert!(inj.dir.is_none());
    }

    #[test]
    fn setup_zsh_writes_redirectors_and_points_zdotdir_at_them() {
        let inj = setup_zsh().expect("zsh setup should succeed");
        let dir = inj.dir.clone().expect("zsh needs a throwaway dir");
        assert_eq!(
            inj.env.get("ZDOTDIR").map(String::as_str),
            Some(dir.to_string_lossy().as_ref())
        );
        assert!(!inj.replaces_argv);
        assert!(inj.args.is_empty());
        for (name, body) in zsh_redirectors() {
            let written = std::fs::read_to_string(dir.join(name)).expect("redirector written");
            assert_eq!(written, body);
        }
        assert!(is_our_zdotdir(&dir.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_bash_writes_rcfile_and_forces_non_login() {
        let inj = setup_bash().expect("bash setup should succeed");
        let dir = inj.dir.clone().expect("bash needs a throwaway dir");
        assert_eq!(inj.args[0], "--rcfile");
        assert_eq!(inj.args[2], "-i");
        assert!(inj.replaces_argv);
        let rc = std::fs::read_to_string(&inj.args[1]).expect("rcfile written");
        assert_eq!(rc, bash_rcfile());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_dispatches_by_shell_and_sets_sentinel() {
        let inj = setup(Some("zsh"), &[], false).expect("zsh setup");
        assert_eq!(
            inj.env.get("LYREL_SHELL_INTEGRATION").map(String::as_str),
            Some("")
        );
        if let Some(d) = inj.dir {
            let _ = std::fs::remove_dir_all(d);
        }

        assert!(setup(Some("zsh"), &[], true).is_none());

        let inj = setup(Some("fish"), &[], false).expect("fish setup");
        assert!(inj.env.contains_key("LYREL_SHELL_INTEGRATION"));

        assert!(setup(Some("fish"), &[], true).is_none());

        let bash = if cfg!(windows) {
            "C:/Program Files/Git/bin/bash.exe"
        } else {
            "bash"
        };
        let inj = setup(Some(bash), &[], false).expect("bash setup");
        assert!(inj.replaces_argv);
        assert!(inj.env.contains_key("LYREL_SHELL_INTEGRATION"));
        if let Some(d) = inj.dir {
            let _ = std::fs::remove_dir_all(d);
        }

        assert!(setup(Some(bash), &[], true).is_none());

        let inj = setup(Some("powershell.exe"), &[], false).expect("powershell setup");
        assert!(inj.env.contains_key("LYREL_SHELL_INTEGRATION"));
        assert!(inj.dir.is_none());
        assert!(!inj.replaces_argv);

        assert!(setup(Some("pwsh"), &[], true).is_none());

        let inj = setup(Some("nu.exe"), &[], false).expect("nushell setup");
        assert!(inj.env.contains_key("LYREL_SHELL_INTEGRATION"));
        assert_eq!(inj.args[0], "--config");
        assert!(!inj.replaces_argv);
        if let Some(d) = inj.dir {
            let _ = std::fs::remove_dir_all(d);
        }

        assert!(setup(Some("nu.exe"), &[], true).is_none());

        assert!(setup(Some("/bin/sh"), &[], false).is_none());
    }

    #[test]
    fn setup_powershell_injects_encoded_command_without_files() {
        let inj = setup_powershell().expect("powershell injection is infallible");
        assert_eq!(inj.args[0], "-NoLogo");
        assert_eq!(inj.args[1], "-NoExit");
        assert_eq!(inj.args[2], "-EncodedCommand");
        assert_eq!(inj.args.len(), 4);
        let b64 = &inj.args[3];
        assert!(
            b64.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='),
            "encoded command must be pure base64"
        );
        assert_eq!(decode_utf16le_base64(b64), POWERSHELL_INTEGRATION);
        assert!(inj.env.is_empty());
        assert!(inj.dir.is_none());
        assert!(!inj.replaces_argv);
    }

    #[test]
    fn powershell_integration_sets_an_osc_title() {
        let s = POWERSHELL_INTEGRATION;
        assert!(s.contains("]0;$($global:__LyrelUser)@$($global:__LyrelHost):"));
        assert!(s.contains("$titlePath = '~'"));
        assert!(s.contains("$titlePath = $fsPath.Replace('\\', '/')"));
    }

    /// `$env:USERNAME`, `$env:COMPUTERNAME` and `$env:USERPROFILE` are Windows-only
    /// spellings that are simply empty on macOS and Linux — reaching for any of
    /// them is what titled every Unix pwsh pane `@:` plus a raw absolute path
    /// (#583). The script runs unchanged on all three platforms, so the cheapest
    /// guard is to keep those names out of it entirely.
    #[test]
    fn powershell_integration_reads_identity_platform_neutrally() {
        let s = POWERSHELL_INTEGRATION;
        for windows_only in ["$env:USERNAME", "$env:COMPUTERNAME", "$env:USERPROFILE"] {
            assert!(
                !s.contains(windows_only),
                "{windows_only} is empty on macOS and Linux; use the .NET \
                 equivalent so the one script is right on every platform"
            );
        }
        assert!(s.contains("[Environment]::UserName"));
        assert!(s.contains("[Environment]::MachineName"));
        assert!(s.contains("$global:__LyrelHome = if ($HOME)"));
    }

    /// A bare `StartsWith($home)` also matches the sibling directory whose name
    /// merely begins with the home directory's, so `/Users/annex` used to be
    /// retitled `~ex`. The separator has to be part of the match.
    #[test]
    fn powershell_home_abbreviation_requires_a_path_separator() {
        let s = POWERSHELL_INTEGRATION;
        assert!(s.contains("$titlePath.StartsWith($global:__LyrelHome + '/')"));
        assert!(s.contains("$titlePath -eq $global:__LyrelHome"));
    }

    #[test]
    fn base64_encode_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn powershell_encoded_command_round_trips_utf16le() {
        let script = "Write-Host 'héllo ✓'";
        assert_eq!(
            decode_utf16le_base64(&powershell_encoded_command(script)),
            script
        );
    }

    pub(super) fn decode_utf16le_base64(b64: &str) -> String {
        fn val(c: u8) -> Option<u32> {
            match c {
                b'A'..=b'Z' => Some((c - b'A') as u32),
                b'a'..=b'z' => Some((c - b'a' + 26) as u32),
                b'0'..=b'9' => Some((c - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        let mut bytes = Vec::new();
        let mut acc = 0u32;
        let mut nbits = 0;
        for c in b64.bytes() {
            let Some(v) = val(c) else { continue };
            acc = (acc << 6) | v;
            nbits += 6;
            if nbits >= 8 {
                nbits -= 8;
                bytes.push((acc >> nbits) as u8);
            }
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        String::from_utf16(&units).expect("valid UTF-16LE")
    }

    #[test]
    fn throwaway_dir_is_unique_per_call() {
        let a = throwaway_dir("lyrel-test-").expect("dir a");
        let b = throwaway_dir("lyrel-test-").expect("dir b");
        assert_ne!(a, b);
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("lyrel-test-")
        );
        assert!(a.is_dir() && b.is_dir());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn setup_nushell_writes_config_and_points_at_it() {
        let inj = setup_nushell().expect("nushell setup should succeed");
        let dir = inj.dir.clone().expect("nushell needs a throwaway dir");
        assert_eq!(inj.args[0], "--config");
        assert_eq!(inj.args.len(), 2);
        let written = std::fs::read_to_string(&inj.args[1]).expect("config written");
        assert_eq!(
            written,
            nushell_config_script_with(nushell_user_config_path().as_deref())
        );
        assert!(inj.env.is_empty());
        assert!(!inj.replaces_argv);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nushell_integration_emits_every_osc_133_mark_and_cwd() {
        let s = NUSHELL_INTEGRATION;
        for mark in ["133;A", "133;B", "133;C", "133;D;"] {
            assert!(s.contains(mark), "Nushell must emit {mark:?}");
        }
        assert!(s.contains("7;file://"));
        assert!(
            s.contains(r#"($nu.os-info.name? | default '') == 'windows'"#),
            "the backslash translation must be gated on Windows — a Unix path \
             may legally contain a literal backslash"
        );
        assert!(
            s.contains(r#"str replace -a '%' '%25'"#),
            "must percent-escape"
        );
        assert!(s.contains("__lyrel_cmd_active"));
        assert!(s.contains("hide-env __lyrel_cmd_active"));
        assert!(s.contains("prompt_indicator?"));
        assert!(
            s.contains(r##"($env.LYREL_SHELL_INTEGRATION? | default "") == """##),
            "must guard install on the sentinel being empty, like the other shells"
        );
    }

    #[test]
    fn nushell_config_script_substitutes_a_literal_source_or_a_noop() {
        let with = nushell_config_script_with(Some(Path::new(
            r"C:\Users\ann\AppData\Roaming\nushell\config.nu",
        )));
        assert!(with.contains("source 'C:\\Users\\ann\\AppData\\Roaming\\nushell\\config.nu'"));
        let source_at = with.find("source ").expect("sources the user config");
        let hooks_at = with.find("hooks.pre_prompt").expect("appends hooks");
        assert!(
            source_at < hooks_at,
            "the user config must be sourced before the hooks are appended"
        );

        let without = nushell_config_script_with(None);
        assert!(
            !without.contains("source '") && !without.contains("source \""),
            "no config.nu means nothing to source"
        );
        assert!(without.contains("no user config.nu to restore"));
        assert!(without.contains("hooks.pre_prompt"));
    }

    #[test]
    fn nu_string_literal_quotes_paths_for_the_wrapper() {
        assert_eq!(
            nu_string_literal(r"C:\Users\ann\nushell\config.nu"),
            r"'C:\Users\ann\nushell\config.nu'"
        );
        assert_eq!(
            nu_string_literal("/home/ann/.config/nushell/config.nu"),
            "'/home/ann/.config/nushell/config.nu'"
        );
        // An apostrophe (legal on Windows) forces the double-quoted fallback.
        assert_eq!(
            nu_string_literal(r"C:\it's\config.nu"),
            r#""C:\\it's\\config.nu""#
        );
        // Single quotes are fully literal in Nushell, so `$` needs no escaping
        // there; only an apostrophe forces the double-quoted fallback.
        assert_eq!(
            nu_string_literal(r"C:\a$b\config.nu"),
            r"'C:\a$b\config.nu'"
        );
        assert_eq!(
            nu_string_literal(r"C:\it's\$x\config.nu"),
            r#""C:\\it's\\\$x\\config.nu""#
        );
    }

    /// nu-path resolves the config dir from `$XDG_CONFIG_HOME` on every
    /// platform, but only when it is non-empty *and* absolute. Both halves are
    /// load-bearing: the original Windows arm ignored the variable outright,
    /// and the original non-Windows arm accepted a relative value nu rejects.
    #[test]
    fn nushell_config_dir_follows_nu_s_own_resolution_rules() {
        use std::ffi::OsStr;
        // `/x` is not absolute on Windows (no prefix), so the fixtures have to
        // be shaped for the host or the rule under test is never exercised.
        let abs_xdg = if cfg!(windows) { r"C:\xdg" } else { "/xdg" };
        let platform = || {
            Some(PathBuf::from(if cfg!(windows) {
                r"C:\platform"
            } else {
                "/platform"
            }))
        };
        let want = |base: &str| Some(PathBuf::from(base).join("nushell"));

        // Unset, empty, or relative: nu ignores it and takes the platform dir.
        for ignored in [None, Some(OsStr::new("")), Some(OsStr::new("relative/dir"))] {
            assert_eq!(
                nushell_config_dir_from(ignored, platform()),
                want(&platform().unwrap().to_string_lossy()),
                "XDG_CONFIG_HOME {ignored:?} must not displace the platform dir"
            );
        }

        // Non-empty and absolute: it wins, even where there is no platform dir.
        assert_eq!(
            nushell_config_dir_from(Some(OsStr::new(abs_xdg)), platform()),
            want(abs_xdg)
        );
        assert_eq!(
            nushell_config_dir_from(Some(OsStr::new(abs_xdg)), None),
            want(abs_xdg)
        );
        assert_eq!(nushell_config_dir_from(None, None), None);
    }

    /// The macOS half of the same rule, stated as itself: nu follows the
    /// platform convention there, which is *not* where lyrel keeps its own
    /// config (`~/.config/lyrel`). Reading `~/.config/nushell` on a Mac finds
    /// nothing, and the wrapper then replaces a config it thinks is absent.
    #[cfg(target_os = "macos")]
    #[test]
    fn nushell_config_dir_is_application_support_on_macos() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        assert_eq!(
            platform_config_dir(),
            Some(home.join("Library/Application Support"))
        );
    }

    /// Ask the real binary. A hand-copy of nu's resolution rules is only as
    /// good as the reading behind it — this is the check that fails outright
    /// if the two ever disagree on this machine.
    #[cfg(unix)]
    #[test]
    fn nushell_config_dir_is_what_nu_itself_reports() {
        let Some(nu) = test_nushell_path() else {
            eprintln!("skipping: Nushell not installed");
            return;
        };
        let xdg_absolute = std::env::temp_dir().join("lyrel-nu-xdg");
        let cases: [Option<&std::ffi::OsStr>; 4] = [
            None,
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("relative/dir")),
            Some(xdg_absolute.as_os_str()),
        ];
        for xdg in cases {
            let mut cmd = std::process::Command::new(&nu);
            cmd.args(["-n", "-c", "$nu.default-config-dir"]);
            match xdg {
                Some(v) => cmd.env("XDG_CONFIG_HOME", v),
                None => cmd.env_remove("XDG_CONFIG_HOME"),
            };
            let out = cmd.output().expect("run nu");
            // A relative value makes nu warn before it answers, so take the
            // last line, not the whole of stdout.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let theirs = PathBuf::from(stdout.lines().last().unwrap_or_default().trim());
            let ours = nushell_config_dir_from(xdg, platform_config_dir())
                .expect("this machine has a config dir");
            assert_eq!(
                ours, theirs,
                "lyrel and nu disagree on the config dir for XDG_CONFIG_HOME={xdg:?}"
            );
        }
    }

    /// End to end over a real pty: a user who *has* a config.nu must still
    /// have it after lyrel points `nu --config` at the wrapper. `--config`
    /// replaces the user's config rather than adding to it, so a wrapper that
    /// fails to source it silently strips their prompt, aliases and
    /// keybindings.
    #[cfg(unix)]
    #[test]
    fn nushell_restores_the_user_config_and_still_reports_the_prompt_cycle() {
        let Some(nu) = test_nushell_path() else {
            eprintln!("skipping: Nushell not installed");
            return;
        };
        let nu = nu.to_string_lossy().into_owned();
        let home = tempfile::tempdir().expect("tempdir");
        let user_config = home.path().join("config.nu");
        // Shaped like a real config.nu: a wholesale `$env.config = {...}` (the
        // conventional ending, and the one that would wipe hooks added before
        // it), plus a marker only the user's own file could have set.
        std::fs::write(
            &user_config,
            "$env.config = { show_banner: false }\n$env.LYREL_USER_CONFIG_LOADED = \"yes\"\n",
        )
        .expect("write user config");

        let mut injection = setup_nushell_with(Some(&user_config)).expect("nushell integration");
        // What `setup` does at every spawn boundary, and what this test
        // bypasses by reaching for `setup_nushell_with` directly. Without it
        // the wrapper is a no-op whenever the test itself runs inside an
        // integrated pane — the guard sees the inherited `1` and skips the
        // whole block, and the marks in the transcript are then Nushell's own.
        injection
            .env
            .insert("LYREL_SHELL_INTEGRATION".to_string(), String::new());
        let text = prompt_cycle_over_pty(
            &nu,
            &injection,
            // One line: Nushell's line editor holds the tty in raw mode and
            // drops what arrives while a command is running, so a second `\r`
            // never reaches it and its prompt cycle never happens. `1 / 0` is
            // the failure — a bare `false` in Nushell is a value.
            b"print $\"probe=($env.LYREL_USER_CONFIG_LOADED? | default 'MISSING')\"; 1 / 0\r",
            Some(home.path()),
        );

        assert!(
            text.contains("probe=yes"),
            "the user's own config.nu must survive --config; got:\n{text}"
        );
        // Assert on lyrel's *own* marks, not on the substring `133;A`: stock
        // Nushell emits its own OSC 133 cycle alongside these, so a bare
        // substring passes even when the wrapper never ran. lyrel's hooks
        // terminate with BEL and Nushell's with ST, which tells them apart.
        for mark in ["133;A\u{7}", "133;C;;aid=lyrel\u{7}", FAILED_COMMAND_MARK] {
            assert!(
                text.contains(mark),
                "Nushell must still report {mark:?} after sourcing the user config; got:\n{text}"
            );
        }
        // Same reasoning for the cwd: the hook falls back to `localhost` for
        // the host, while Nushell's native OSC 7 names the real one.
        assert!(
            text.contains("7;file://localhost/"),
            "lyrel's own OSC 7 must be among the reports; got:\n{text}"
        );
        assert_eq!(
            last_osc7(&text),
            home.path().canonicalize().expect("canonical tempdir"),
            "the pane's reported cwd must be where nu actually is"
        );
    }

    /// `shells::nushell_path` is Windows-only (nu has no fixed install path
    /// there); on unix the binary is whatever `nu` resolves to on PATH.
    #[cfg(unix)]
    fn test_nushell_path() -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join("nu"))
            .find(|p| p.is_file())
    }

    #[cfg(windows)]
    #[test]
    fn nushell_reports_the_full_prompt_cycle_over_a_real_pty() {
        let Some(nu) = crate::core::shells::nushell_path() else {
            eprintln!("skipping: Nushell not installed");
            return;
        };
        let nu = nu.to_string_lossy().into_owned();
        let injection = setup(Some(&nu), &[], false).expect("nushell integration");
        // One line: Nushell's line editor holds the tty in raw mode and can
        // drop what arrives while a command is running, so a second `\r` may
        // never reach it. `cd` first, then a command that fails — `1 / 0`
        // errors with exit status 1 (a literal `false` is just a value).
        let text = prompt_cycle_over_pty(
            &nu,
            &injection,
            b"cd C:/Windows; 1 / 0\r",
            Some(Path::new("C:/")),
        );

        for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
            assert!(
                text.contains(mark),
                "Nushell must report {mark:?}; got:\n{text}"
            );
        }
        let cwd = reported_cwd(&text);
        assert!(
            cwd.exists(),
            "Nushell reported a cwd the Windows side cannot resolve: {cwd:?} \
             — a drive-relative path, so the URI leading-slash translation regressed"
        );
        assert_eq!(
            last_osc7(&text),
            PathBuf::from("C:/Windows"),
            "`cd` must move the pane's reported cwd; got:\n{text}"
        );
    }
}
