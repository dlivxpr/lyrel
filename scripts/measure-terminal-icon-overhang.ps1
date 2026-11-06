param(
    [Parameter(Mandatory=$true)][string]$Screenshot,
    [Parameter(Mandatory=$true)][double]$OriginX,
    [Parameter(Mandatory=$true)][double]$OriginY,
    [Parameter(Mandatory=$true)][double]$CellWidth,
    [Parameter(Mandatory=$true)][double]$LineHeight,
    [switch]$VerifyMatching
)
# 配合 terminal-icon-overhang.mjs；坐标是所保存 PNG 的像素，不是逻辑坐标。
# 只读自有窗口截图。先确保标题不换行、网格没有滚动、未选择任何文字。
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$bitmap = [System.Drawing.Bitmap]::FromFile((Resolve-Path $Screenshot))
try {
    $samples = @()
    foreach ($case in @(
        @{ Name='default'; Row=8 },
        @{ Name='matching-background'; Row=9 },
        @{ Name='nonblank'; Row=10 },
        @{ Name='mismatched-background'; Row=11 },
        @{ Name='icon-only-background'; Row=12 },
        @{ Name='blank-only-background'; Row=13 }
    )) {
        foreach ($ix in 0..3) {
            $x0 = [int][Math]::Ceiling($OriginX + (24 + 4 * $ix) * $CellWidth)
            $x1 = [int][Math]::Floor($OriginX + (26 + 4 * $ix) * $CellWidth - 1)
            $y0 = [int][Math]::Ceiling($OriginY + $case.Row * $LineHeight)
            $y1 = [int][Math]::Floor($OriginY + ($case.Row + 1) * $LineHeight - 1)
            if ($x0 -lt 0 -or $y0 -lt 0 -or $x1 -ge $bitmap.Width -or $y1 -ge $bitmap.Height) {
                throw 'ROI 超出截图：请使用当前 PNG 的真实网格起点和 cell metrics。'
            }
            $left=$x1+1; $right=$x0-1; $top=$y1+1; $bottom=$y0-1; $count=0
            foreach ($y in $y0..$y1) {
                foreach ($x in $x0..$x1) {
                    $pixel = $bitmap.GetPixel($x,$y)
                    if ($pixel.R -gt 140 -and $pixel.B -gt 140 -and $pixel.G -lt 110) {
                        $left=[Math]::Min($left,$x); $right=[Math]::Max($right,$x)
                        $top=[Math]::Min($top,$y); $bottom=[Math]::Max($bottom,$y)
                        $count++
                    }
                }
            }
            $samples += [pscustomobject]@{
                case=$case.Name; icon=@('U+F002','U+F179','U+F059','U+F07C')[$ix]
                roi=@($x0,$y0,$x1,$y1); pixels=$count
                inkBounds=$(if ($count) {@($left,$top,$right,$bottom)} else {$null})
                inkWidth=$(if ($count) {$right-$left+1} else {0})
                inkHeight=$(if ($count) {$bottom-$top+1} else {0})
            }
        }
    }
    [pscustomobject]@{
        screenshot=(Resolve-Path $Screenshot).Path
        pixelSize=@($bitmap.Width,$bitmap.Height)
        grid=@($OriginX,$OriginY,$CellWidth,$LineHeight)
        evidenceBoundary='仅统计未选择的 magenta ink；nonblank ROI 包含 X。选区、搜索、命中、滚动与其他平台须单独验收。'
        samples=$samples
    } | ConvertTo-Json -Depth 6
    if ($VerifyMatching) {
        $eligible = 0
        foreach ($icon in @('U+F002','U+F179','U+F059','U+F07C')) {
            $plain = $samples | Where-Object { $_.case -eq 'default' -and $_.icon -eq $icon }
            $matching = $samples | Where-Object { $_.case -eq 'matching-background' -and $_.icon -eq $icon }
            if ($plain.inkWidth -gt $CellWidth + 2) {
                $eligible++
                if ($matching.inkWidth -lt $plain.inkWidth - 2 -or $matching.inkHeight -lt $plain.inkHeight - 2) {
                    throw "$icon 的匹配背景仍缩小：$($plain.inkWidth)x$($plain.inkHeight) -> $($matching.inkWidth)x$($matching.inkHeight)"
                }
            }
        }
        if (-not $eligible) { throw '没有可判定的超单格字形：请检查字体、坐标与截图，不得将本次结果记为通过。' }
    }
} finally { $bitmap.Dispose() }
