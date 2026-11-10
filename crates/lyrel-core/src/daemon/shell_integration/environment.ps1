# Windows PowerShell / older .NET delete empty values through Environment.Set.
# The native setter preserves an empty value as distinct from an absent key.
function global:__LyrelScopeSet([string]$Key, $Value) {
  if ($null -ne $Value -and $Value -eq '' -and $env:OS -eq 'Windows_NT' -and $PSVersionTable.PSVersion -lt [version]'7.5') {
    if (-not ('LyrelScope.NativeEnvironment' -as [type])) {
      Add-Type -Name NativeEnvironment -Namespace LyrelScope -MemberDefinition '[System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)] public static extern bool SetEnvironmentVariable(string name, string value);'
    }
    if (-not [LyrelScope.NativeEnvironment]::SetEnvironmentVariable($Key, [string]$Value)) { throw 'cannot preserve empty process environment value' }
  } else {
    [Environment]::SetEnvironmentVariable($Key, $Value, 'Process')
  }
}
