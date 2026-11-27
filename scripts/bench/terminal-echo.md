# Lyrel 纯文本与前台按键回显（#430）

每轮使用独占、未锁屏的实机、独立 Release 构建、独立配置和专用 daemon。准备好产品、CLI、测试 fixture 与探针后再开始采样。暂停其他构建、测试和 GPU 工作，记录提交、工具版本、CPU/GPU/内存、显示驱动、刷新率、HDR/VRR/DPI、窗口与终端网格、字体、输入法版本和实际回放命令。固定这些设置，依次交替运行 baseline/candidate，每种场景至少各三轮，保留每轮数据、匹配覆盖率及轮间变化。比较时用同一版本的测量工具和固定 fixture。使用至少 83 列以避免工作量换行，校准后不移动或调整窗口。

### 准备 Windows 环境

使用 Windows x64、已准备固定 ConPTY 文件的 Release `lyrel-app.exe`、`lyrel.exe` 和 `lyrel-graphics-fixture.exe`。以具有 Visual Studio C++ x64 工具链及支持 Windows Graphics Capture（WGC）的 Windows SDK 的 PowerShell 运行，`INCLUDE` 需要包含 SDK 的 `cppwinrt` 目录。校准截图复用 `ffmpeg` 9.0.2 的 `gfxcapture`。显示时序复用 PresentMon 2.6.0，需已有 ETW 访问权限，并记录其版本与 SHA-256。Node.js 运行离线关联脚本。

```powershell
pwsh scripts/prepare-conpty.ps1
pwsh scripts/cargo-windows.ps1 build --locked --release -p lyrel -p lyrel-cli -p lyrel-test-support --bin lyrel-app --bin lyrel --bin lyrel-graphics-fixture
$repo = (Get-Location).Path
$build = Join-Path $env:TEMP ('lyrel-echo-tools-' + [guid]::NewGuid())
New-Item -ItemType Directory $build | Out-Null
Push-Location $build
try {
  cl.exe /nologo /std:c++20 /EHsc /O2 "$repo/scripts/bench/terminal-echo-windows.cpp" /link d3d11.lib windowsapp.lib user32.lib
  if ($LASTEXITCODE -ne 0) { throw 'Echo probe build failed' }
} finally { Pop-Location }
$probe = Join-Path $build 'terminal-echo-windows.exe'
```

### 启动每轮固定工作量

在专用测试账户的 PowerShell 中设置下列进程级环境变量。每一轮必须为 `$dir` 生成新目录。用 Cargo wrapper 报告的实际产物路径替换示例中的 `target/release`。保持 `$env:LYREL_CONFIG_DIR` 等参数一致，以便第二个采样终端的产品 CLI 连接本轮专用 daemon。

```powershell
$target = (Resolve-Path 'target/release').Path
$appBin = Join-Path $target 'lyrel-app.exe'
$cli = Join-Path $target 'lyrel.exe'
$fixture = Join-Path $target 'lyrel-graphics-fixture.exe'
$dir = Join-Path $env:TEMP ('lyrel-echo-' + [guid]::NewGuid())
New-Item -ItemType Directory $dir | Out-Null
'config','data','front','background','workspace','evidence' | ForEach-Object {
  New-Item -ItemType Directory (Join-Path $dir $_) | Out-Null
}
$env:LYREL_CONFIG_DIR = Join-Path $dir 'config'
$env:LYREL_DATA_DIR = Join-Path $dir 'data'
@{
  shell = @{ program = $fixture; args = @('plaintext', "$dir/front", '400') }
  custom_shells = @(@{ label = 'Plaintext background'; program = $fixture; args = @('plaintext', "$dir/background", '400') })
  font_family = 'Hack'
  font_size = 16
  scrollback_limit = 15000
  window_opacity = 1
  theme = 'dark'
  theme_preset = 'dark'
  install_cli_on_path = $false
  check_for_updates = $false
  auto_download_updates = $false
  show_tray_icon = $false
  theme_follow_system = $false
} | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 "$env:LYREL_CONFIG_DIR/config.json"
$daemon = Start-Process $appBin -ArgumentList '--daemon' -WorkingDirectory "$dir/workspace" -PassThru
```

等候 `$env:LYREL_CONFIG_DIR` 下的 `control.port` 与 `daemon.port` 建立。随后在同一 PowerShell 中运行以下命令。核对 GUI 和 daemon 的 PID、程序路径和隔离配置。确认 GUI 显示夹具的 `PLAIN_READY`，在运行探针前保持终端的英文输入模式、Caps Lock 关闭，并且无需手动向夹具发送键。

```powershell
$app = Start-Process $appBin -WorkingDirectory "$dir/workspace" -PassThru
$deadline = (Get-Date).AddSeconds(30)
do { Start-Sleep -Milliseconds 100; $app.Refresh() }
until ($app.MainWindowHandle -ne 0 -or (Get-Date) -gt $deadline)
if ($app.MainWindowHandle -eq 0) { throw 'Owned GUI HWND unavailable' }
$hwnd = $app.MainWindowHandle.ToInt64()
Get-Process -Id $app.Id,$daemon.Id | Select-Object Id,Path,MainWindowHandle
```

同样的前台 pane 可用于三种场景及其他竞争工作量。

使用 `capture --plain` 做逐行校验时，先确认产生输出的窗格完成首次 resize，且生产全程至少为 83 列。Windows 夹具关闭自动换行，初始 80 列网格会覆写超宽行的末列。需要校验 PTY 字节时用原始 `capture --scrollback`。

| 场景 | 探针的最后一个参数 |
| --- | --- |
| 空闲 | 省略。前台 fixture 等候 `start` 文件，只有回显 |
| 可见输出 | `"$dir/front/start"`。发送的键和生产输出共用该窗格 |
| 隐藏输出 | `"$dir/background/start"`。在新建标签菜单选择 `Plaintext background`，看到 `PLAIN_READY` 后切回前台标签。采样后返回后台标签核对 `PLAIN 9600` 和 `PLAIN_DONE`，再在后台窗格输入 `a`，屏幕应出现 `>aaab>a>` |
| 其他竞争工作量 | 省略 `start` 参数。让第二工作量在另一窗格或进程等候独立启动信号，在探针运行时启动，记录其起止及与输入样本的交集 |

负载以每批 16 行、每秒 400 行、每 40 ms 绝对截止时间调度。一轮共 600 批、9,600 行，每行 82 字节加 CRLF，合计 806,400 字节。报告 `producer.csv` 最后一批的 `flushed_ms`、与 `scheduled_ms` 的偏差及总时长，`PLAIN_DONE` 认证生产结束。校准行、回显、IME 和完成标记不计入语料的字节数。

### 采集按键与首个正确帧

采集只针对本轮已核对的 HWND。先用现有 WGC 截图入口获取校准图。按物理像素填写终端第一列第一行左上角、单元格宽度、行高与可见行数。测量相同字形的相邻起点间距可得到单元格尺寸，将连续多个单元格的距离除以间隔数可保留小数。终端使用不透明的亮字深背景，保留截图中的最初五行校准字形。截图与持续探针都无需读取用户文件。

```powershell
ffmpeg -hide_banner -n -filter_complex "gfxcapture=hwnd=${hwnd}:max_framerate=1:capture_cursor=0:display_border=0,hwdownload,format=bgra" -frames:v 1 "$dir/evidence/calibration.png"
$x,$y,$cellW,$cellH,$rows = (Read-Host '校准图中的 X,Y,单元格宽度,行高,行数') -split ','
$refreshHz = Read-Host '窗口所在显示器实际且固定的刷新率'
$pm = Read-Host 'PresentMon 2.6.0 的绝对路径'
Get-FileHash -Algorithm SHA256 $pm
```

在有 ETW 采集权限的第二个采样终端启动 PresentMon，仅采集本轮 PID。先确认夹具窗格使用英语直输布局（例如 English (United States)），并在采样开始前确认 UAC 已关闭、焦点回到同一个 HWND。若权限拒绝，记录错误并在分析命令中省略 `--presentmon`。

```powershell
& $pm --process_id $app.Id --v2_metrics --qpc_time_ms --write_display_metadata `
  --session_name "lyrel430-$($app.Id)" --timed 45 --terminate_after_timed `
  --no_console_stats --output_stdout > "$dir/evidence/presentmon.csv" 2> "$dir/evidence/presentmon.log"
$LASTEXITCODE | Set-Content "$dir/evidence/presentmon.exit"
```

确认 CSV 有本轮 PID 的数据行。PresentMon 退出码 0 仍可能只有空文件。若截图和 PTY 显示按键停留在 IME 预编辑中，保留失败样本并用新窗格复试。

随后运行下面的探针，在四秒内把焦点放到本轮前台终端并停止手工输入。示例 80 次、间隔 250 ms，前台采样位于固定负载约 24 秒内。用表格中的最后一个参数选择场景。程序逐次检查前台 HWND 与 PID，捕获几何变化或焦点丢失时本轮失败。`inputs.csv` 的时间戳在 Windows `SendInput` 调用前读取 QPC，`frames.csv` 直接使用每个 WGC 框的 `SystemRelativeTime`。`>序号>键>` 的八个字形含真实回显字母，序号由四位 `a..d` 编码。探针比较同一窗口校准像素，仅保存字符身份与时间戳。校准 PNG 仅含合成 fixture 数据。

```powershell
Push-Location "$dir/evidence"
try {
  & $probe $hwnd $app.Id $x $y $cellW $cellH $rows 80 250 "$dir/front/start"
  if ($LASTEXITCODE -ne 0) { throw 'Echo probe failed' }
} finally { Pop-Location }
node scripts/terminal-display-analysis.mjs --inputs "$dir/evidence/inputs.csv" `
  --frames "$dir/evidence/frames.csv" --presentmon "$dir/evidence/presentmon.csv" `
  --pid $app.Id --refresh-hz $refreshHz > "$dir/evidence/latency.json"
```

分析输出包含 WGC 首次识别的 p50/p95、关联的显示时延 p50/p95、超出两个刷新周期的显示间隔、未显示和未配对样本。显示关联限同一 PID 的单个 swapchain。先用上一个 WGC 帧排除已显示旧内容的事件，剩余事件必须唯一，在输入之后且与正确 WGC 时间的距离不超过半个刷新周期。不同 WGC 帧若指向同一次呈现，两者都记作 `shared_present`。同一 WGC 帧可以同时确认多个标记。多个 swapchain 需提供独立证据选定 `--swapchain`，否则本轮显示关联失败。WGC 是窗口合成内容，时间关联存在采集和时序误差。只有 PresentMon 支持的唯一匹配才能进入显示时延分布。CSV 缺失时仅报告合成时延，显示样本全部 unmatched。分析退出码 `2` 表示存在未匹配或意外标记，`1` 表示输入或分析失败。记录覆盖率，采集丢帧或无法消歧时保留缺失。其他操作可复用同一分析入口，提供 `id,qpc_ms` 输入和正确画面观测。每个输入 ID 唯一，画面 CSV 记录 ID 的首次出现与异常重复，同源校准后的无 ID 帧用空 `id`。

### 正确性、过程计数与清理

两种负载等候 `PLAIN_DONE` 后保存完整 CLI 回放。使用已安装的中文 IME 对前台终端提交 `中文` 一次，再开始另一段组字并按 Esc 取消。直接确认前台 `IME 中`、`IME 文` 各出现一次，取消内容不会进入回显。先完成拉丁按键采样再做人工 IME 检查，保留本次平台、IME、操作与结果。隐藏回放需要在恢复标签后亲眼核对最新编号及标记。

```powershell
$frontId = (Get-Content "$dir/front/pane.txt").Trim()
$keys = 'abcd' * 20
& $cli capture "%$frontId" --scrollback --plain > "$dir/evidence/front.txt"
if ($LASTEXITCODE -ne 0) { throw 'Foreground replay failed' }
& $fixture plaintext-check "$dir/evidence/front.txt" load $keys 中文
if ($LASTEXITCODE -ne 0) { throw 'Foreground transcript failed' }
# idle / 隐藏输出时，前台验证将 load 改为 idle。
# 仅隐藏输出额外验证后台。先恢复标签、核对完成并发送一次 a。
$backgroundId = (Get-Content "$dir/background/pane.txt").Trim()
& $cli capture "%$backgroundId" --scrollback --plain > "$dir/evidence/background.txt"
if ($LASTEXITCODE -ne 0) { throw 'Hidden pane replay failed' }
& $fixture plaintext-check "$dir/evidence/background.txt" load a
if ($LASTEXITCODE -ne 0) { throw 'Hidden pane transcript failed' }
```

采样时在独立采样终端用 `sample-windows-resources.ps1` 记录本轮 GUI 和 daemon 的确切 PID。活动的生产者 PID 在 `$dir/front/pid.txt` 或 `$dir/background/pid.txt`。每个角色报告私有字节、工作集、CPU 秒与归一化值 `CPU 秒 / 实际行数 × 10,000`，空闲只报告 CPU 秒或逻辑核心用量。GUI GPU 用 PID 对应最忙引擎的利用率。每轮记录启动、峰值、稳态以及关闭窗格后固定间隔的内存，并在同一 GUI/daemon 寿命内重复新建、播放及关闭窗格。为每个重复周期分配新的 fixture 目录，并预先添加到 `custom_shells`。采样完成仅关闭本轮 GUI 与专用 daemon，保留本轮日志和数据。若缺少权限或适用计数器，记录不可用及具体错误。

Windows 与 macOS 可用 native consumer 验证同一配方。Windows 用 Cargo wrapper 执行同样参数。真实原生 IME 走上面的操作步骤，macOS 另需只捕获自有窗口且保留共同单调时钟的显示证据。

自动用例核对整份保留缓冲区的输出顺序、回显与 IME 提交次数，另外在可见网格核对首键、负载输出和负载结束后的输入。高速输出期间，回显可能在下一次画面采样前滚出可见网格。物理显示时延仍由前台探针和显示事件共同验证。

```sh
cargo test --locked -p lyrel-test-support --lib corpus_and_echo_certificates_reject_loss_reordering_and_duplicates
cargo test --locked -p lyrel --features native-markdown-tests --test native_terminal -- plaintext_echo
node --test scripts/terminal-display-analysis.test.mjs
```
