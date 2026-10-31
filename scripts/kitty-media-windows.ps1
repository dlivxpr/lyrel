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
$oldTreat = [Console]::TreatControlCAsInput
$ids = 14501,14502,14503,14504,14505,14506,14507
function Send-Kitty([string]$header, [string]$name, [string]$expected) {
    $payload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($name))
    [Console]::Write("`e_G$header;$payload`e\")
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $answer = ''
    while (-not $answer.EndsWith("`e\")) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "Protocol response deadline: $header" }
        if ([Console]::KeyAvailable) { $answer += [Console]::ReadKey($true).KeyChar }
        else { [Threading.Thread]::Sleep(2) }
    }
    if ($answer -cne $expected) { throw "Unexpected correlated response: $([Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes($answer)))" }
}
try {
    [Console]::TreatControlCAsInput = $true
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
        while ([Console]::ReadKey($true).Key -ne [ConsoleKey]::Enter) {}
    }
} finally {
    foreach ($id in $ids) { [Console]::Write("`e_Ga=d,d=I,i=$id,q=2`e\") }
    if ($mapping) { $mapping.Dispose() }
    [Console]::TreatControlCAsInput = $oldTreat
    # Root is created by this exact invocation and never supplied by terminal output.
    if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root, $true) }
}
