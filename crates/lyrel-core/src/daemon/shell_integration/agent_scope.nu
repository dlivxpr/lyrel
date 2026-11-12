# Definitions live at config top level: definitions inside an `if` would not
# survive into the interactive session. ^ bypasses only these local wrappers;
# normal external executable/PATH resolution and native argument lists remain.
def __lyrel_agent_environment [agent: string] {
  let identity = if (not ('TERM_PROGRAM' in $env) or $env.TERM_PROGRAM == 'lyrel') {
    {TERM_PROGRAM: 'kitty', TERM_PROGRAM_VERSION: '', LC_TERMINAL: 'lyrel', LC_TERMINAL_VERSION: ($env.LYREL? | default '')}
  } else { {} }
  let identity = if $agent == 'omp' {
    # Same default as the POSIX/PowerShell/fish hooks: an inherited value is
    # not an explicit command-line opt-out, so omp still opts out of DECCARA.
    $identity | insert PI_NO_DECCARA '1'
  } else { $identity }
  let identity = if $agent == 'omp' and ('LYREL_OMP_IMAGE_CONFIG' in $env) and ($env.LYREL_OMP_IMAGE_CONFIG | is-not-empty) {
    let separator = if $nu.os-info.name == 'windows' { ';' } else { ':' }
    let existing = ($env.PI_CONFIG_FILES? | default '')
    let overlays = if ($existing | is-empty) { $env.LYREL_OMP_IMAGE_CONFIG } else { $existing + $separator + $env.LYREL_OMP_IMAGE_CONFIG }
    $identity | insert PI_CONFIG_FILES $overlays
  } else { $identity }
  if $agent == 'hermes' and not ('KITTY_WINDOW_ID' in $env) {
    $identity | insert KITTY_WINDOW_ID 'lyrel'
  } else { $identity }
}
def --wrapped omp [...args] {
  with-env (__lyrel_agent_environment 'omp') { ^omp ...$args }
}
def --wrapped kimi [...args] {
  with-env (__lyrel_agent_environment 'kimi') { ^kimi ...$args }
}
def --wrapped pi [...args] {
  with-env (__lyrel_agent_environment 'pi') { ^pi ...$args }
}
def --wrapped opencode [...args] {
  with-env (__lyrel_agent_environment 'opencode') { ^opencode ...$args }
}
def --wrapped opencode2 [...args] {
  with-env (__lyrel_agent_environment 'opencode2') { ^opencode2 ...$args }
}
def --wrapped hermes [...args] {
  with-env (__lyrel_agent_environment 'hermes') { ^hermes ...$args }
}
