# 在自己新建的隔离 lyrel pane 内运行；不连接/关闭其他 pane 或 daemon。
# 它创建并清理自有文件/section，显示三组红绿像素。原生窗口观察仍需人工记录。
[CmdletBinding()]
param([switch]$KeepVisible)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not $env:LYREL_PANE) { throw 'Run pwsh in an owned native Windows lyrel pane.' }
$root = Join-Path ([IO.Path]::GetTempPath()) ('lyrel-media-' + [guid]::NewGuid())
[IO.Directory]::CreateDirectory($root) | Out-Null
$offset = 48MB + 13
$length = 80MB
[byte[]]$pixels = 255,0,0,255,0,255,0,255
$sharedName = 'Local\lyrel-media-' + [guid]::NewGuid()
$mapping = $null
# APC 响应必须通过 raw VT 字节流读取；Console.ReadKey 会解释并吞掉转义序列。
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public sealed class KittyMediaConsole : IDisposable {
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr GetStdHandle(int id);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetConsoleMode(IntPtr handle, out uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleMode(IntPtr handle, uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint timeout);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool ReadFile(IntPtr handle, byte[] buffer, uint count, out uint read, IntPtr overlapped);
    readonly IntPtr input = GetStdHandle(-10);
    readonly uint mode;
    readonly byte[] buffer = new byte[1];
    public KittyMediaConsole() {
        if (!GetConsoleMode(input, out mode) || !SetConsoleMode(input, 0x200))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }
    public int ReadByte(uint timeout) {
        uint wait = WaitForSingleObject(input, timeout);
        if (wait == 258) throw new TimeoutException("Kitty input deadline");
        if (wait != 0 || !ReadFile(input, buffer, 1, out uint read, IntPtr.Zero))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        if (read != 1) throw new InvalidOperationException("Kitty input EOF");
        return buffer[0];
    }
    public void Dispose() {
        if (!SetConsoleMode(input, mode)) throw new Win32Exception(Marshal.GetLastWin32Error());
    }
}
'@
$console = $null
$ids = 14501,14502,14503,14504,14505,14506,14507
function Send-Kitty([string]$header, [string]$name, [string]$expected) {
    $payload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($name))
    [Console]::Write("`e_G$header;$payload`e\")
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $answer = ''
    while (-not $answer.EndsWith("`e\", [StringComparison]::Ordinal)) {
        $remaining = [Math]::Ceiling(($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if ($remaining -le 0 -or $answer.Length -ge 4096) { throw "Protocol response deadline/limit: $header" }
        $answer += [char]$console.ReadByte([uint32]$remaining)
    }
    if (-not [String]::Equals($answer, $expected, [StringComparison]::Ordinal)) { throw "Unexpected correlated response: $([Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes($answer)))" }
}
try {
    $console = [KittyMediaConsole]::new()
    $regular = Join-Path $root 'regular.rgba'
    $temp = Join-Path $root 'tty-graphics-protocol-temp.rgba'
    foreach ($path in $regular,$temp) {
        $file = [IO.File]::Open($path, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
        try { $file.SetLength($length); $file.Position = $offset; $file.Write($pixels, 0, $pixels.Length) }
        finally { $file.Dispose() }
    }
    $mapping = [IO.MemoryMappedFiles.MemoryMappedFile]::CreateNew($sharedName, $length)
    $writer = $mapping.CreateViewAccessor($offset, 8, [IO.MemoryMappedFiles.MemoryMappedFileAccess]::Write)
    try { $writer.WriteArray(0, $pixels, 0, 8) } finally { $writer.Dispose() }
    [Console]::Write("`e[2J`e[HWindows owned media: each row must be RED then GREEN`r`n")
    foreach ($case in @(@(14501,'f',$regular,3), @(14502,'t',$temp,7), @(14503,'s',$sharedName,11))) {
        $id,$medium,$name,$row = $case
        [Console]::Write("`e[$row;3H")
        Send-Kitty "a=T,i=$id,p=9,t=$medium,f=32,s=2,v=1,O=$offset,S=8,c=12,r=3,C=1" $name "`e_Gi=$id,p=9;OK`e\"
    }
    if (-not [IO.File]::Exists($regular) -or [IO.File]::Exists($temp)) { throw 'Regular/temp lifetime mismatch' }
    $id = 14504
    foreach ($name in @((Join-Path $root 'missing'), 'NUL', '\\.\pipe\lyrel-media-not-owned', '\\localhost\C$\lyrel-media-not-owned')) {
        Send-Kitty "a=q,i=$id,p=9,t=f,f=32,s=2,v=1,S=8" $name "`e_Gi=$id,p=9;EBADF:Failed to read image file`e\"
    }
    # A short requested range and huge to-end request use the same file error.
    Send-Kitty 'a=q,i=14505,p=9,t=f,f=32,s=2,v=1,O=83886076,S=8' $regular "`e_Gi=14505,p=9;EBADF:Failed to read image file`e\"
    Send-Kitty 'a=q,i=14506,p=9,t=f,f=32,s=2,v=1' $regular "`e_Gi=14506,p=9;EBADF:Failed to read image file`e\"
    [Console]::Write("`e[15;1HMEDIA_PROTOCOL_OK: file/temp/shm O/S and uniform refusal. Native pixels are NOT automatically verified.`r`n")
    if ($KeepVisible) {
        [Console]::Write('Inspect the three red/green rows; press Enter to delete fixture images and finish.')
        while ($console.ReadByte(300000) -ne 13) {}
    }
} finally {
    foreach ($id in $ids) { [Console]::Write("`e_Ga=d,d=I,i=$id,q=2`e\") }
    if ($mapping) { $mapping.Dispose() }
    if ($console) { $console.Dispose() }
    # Root is created by this exact invocation and never supplied by terminal output.
    if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root, $true) }
}
