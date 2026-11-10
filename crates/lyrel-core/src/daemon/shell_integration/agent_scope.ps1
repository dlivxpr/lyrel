  # The full submitted AST, never the truncated OSC payload, selects a scope.
  function global:__LyrelScopeRestore {
    if ($null -ne $global:__LyrelScopeSaved) {
      foreach ($key in $global:__LyrelScopeSaved.Keys) {
        __LyrelScopeSet $key $global:__LyrelScopeSaved[$key]
      }
      $global:__LyrelScopeSaved = $null
    }
  }
  function global:__LyrelScopeApply([string]$Line) {
    if ($null -ne $global:__LyrelScopeSaved) { return }
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($Line, [ref]$tokens, [ref]$errors)
    if ($errors.Count -or $ast.BeginBlock -or $ast.ProcessBlock -or $ast.ParamBlock -or $ast.EndBlock.Traps) { return }
    $statements = @($ast.EndBlock.Statements)
    if (-not $statements.Count) { return }
    $explicit = @{}
    # PowerShell's native leading env assignments are separate statements.
    # Only literal env assignments followed by exactly one command qualify.
    for ($i = 0; $i -lt $statements.Count - 1; $i++) {
      $assignment = $statements[$i]
      if ($assignment -isnot [System.Management.Automation.Language.AssignmentStatementAst] -or
          $assignment.Operator -ne 'Equals' -or
          $assignment.Left -isnot [System.Management.Automation.Language.VariableExpressionAst] -or
          $assignment.Left.VariablePath.UserPath -notmatch '^env:([A-Za-z_][A-Za-z_0-9]*)$') { return }
      $key = $Matches[1]
      # AssignmentStatementAst wraps a literal RHS in CommandExpressionAst;
      # SafeGetValue rejects that statement wrapper, not its literal expression.
      $value = $assignment.Right
      if ($value -isnot [System.Management.Automation.Language.CommandExpressionAst] -or $value.Redirections.Count) { return }
      try { $null = $value.Expression.SafeGetValue() } catch { return }
      $explicit[$key] = $true
    }
    $pipeline = $statements[-1]
    if ($pipeline -isnot [System.Management.Automation.Language.PipelineAst] -or $pipeline.PipelineElements.Count -ne 1) { return }
    $command = $pipeline.PipelineElements[0]
    if ($command -isnot [System.Management.Automation.Language.CommandAst] -or $command.Redirections.Count -or
        $command.CommandElements[0] -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) { return }
    if ($command.InvocationOperator -eq 'Dot') { return }
    if ($command.Find({param($node) $node -is [System.Management.Automation.Language.ScriptBlockExpressionAst] -or $node -is [System.Management.Automation.Language.SubExpressionAst]}, $true)) { return }
    $name = $command.GetCommandName()
    if (-not $name) { return }
    if ($name.Contains('/') -or $name.Contains('\')) {
      if (-not [IO.Path]::IsPathRooted($name)) { return }
    }
    $agent = [IO.Path]::GetFileNameWithoutExtension($name).ToLowerInvariant()
    if ([IO.Path]::GetFileName($name) -notmatch '^(omp|kimi|pi|opencode|opencode2|hermes)(\.exe|\.cmd|\.bat|\.ps1)?$') { return }
    $version = if (Test-Path Env:LYREL) { $env:LYREL } else { $global:__LyrelScopeVersion }
    $defaults = @{ TERM_PROGRAM='kitty'; TERM_PROGRAM_VERSION=''; LC_TERMINAL='lyrel'; LC_TERMINAL_VERSION=$version }
    if ($agent -eq 'omp') { $defaults.PI_NO_DECCARA = '1' }
    $environment = [Environment]::GetEnvironmentVariables('Process')
    if ($agent -eq 'hermes' -and -not $environment.Contains('KITTY_WINDOW_ID')) { $defaults.KITTY_WINDOW_ID = 'lyrel' }
    $global:__LyrelScopeSaved = @{}
    foreach ($key in @('TERM_PROGRAM','TERM_PROGRAM_VERSION','LC_TERMINAL','LC_TERMINAL_VERSION','PI_NO_DECCARA','KITTY_WINDOW_ID','OPENCODE_CLI_CONFIG_CONTENT')) {
      $global:__LyrelScopeSaved[$key] = $environment[$key]
    }
    foreach ($key in $defaults.Keys) {
      if (-not $explicit.ContainsKey($key)) { __LyrelScopeSet $key $defaults[$key] }
    }
  }
