# Before PowerShell 7.5, Windows Env: and the .NET setter delete empty values.
# Use the native setter for empty scope values/restoration, but Env: for unset.
function global:__LyrelScopeSet([string]$Key, $Value) {
  # PowerShell 7.5+ coerces $null to an empty string for the .NET setter.
  # Delete through Env: instead, without conflating absence with an empty value.
  if ($null -eq $Value) {
    if (Test-Path -LiteralPath "Env:$Key") { Remove-Item -LiteralPath "Env:$Key" -ErrorAction Stop }
  } elseif ($Value -eq '' -and $env:OS -eq 'Windows_NT' -and $PSVersionTable.PSVersion -lt [version]'7.5') {
    if (-not ('LyrelScope.NativeEnvironment' -as [type])) {
      Add-Type -Name NativeEnvironment -Namespace LyrelScope -MemberDefinition '[System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)] public static extern bool SetEnvironmentVariable(string name, string value);'
    }
    if (-not [LyrelScope.NativeEnvironment]::SetEnvironmentVariable($Key, [string]$Value)) { throw 'cannot preserve empty process environment value' }
  } else {
    [Environment]::SetEnvironmentVariable($Key, $Value, 'Process')
  }
}
