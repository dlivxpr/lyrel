# #434 桌面会话恢复

在 Windows x64 与 macOS 原生桌面运行 `desktop-restore-434.mjs`。场景从一个新建的 `Workspace434` 开始。`Restore434` 有左右两个平铺窗格，`Anchor434` 有一个始终醒着的窗格。三个 Bash 会话分别进入 `left`、`right`、`anchor` 目录，并用键盘输入唯一命令。CLI 只核对会话和终端回放、命名夹具以及停止专用 daemon。受测输入完整经过桌面窗口、终端和真实 Shell。

## 前提与运行

- 可在已有 Lyrel 安装的原生桌面账户运行。脚本只向 GUI、daemon、CLI 和 Bash 夹具传入本轮配置与环境。每轮新建 home、AppData、XDG 和临时目录，并清除继承的代理路径、Lyrel 端点和 Shell 启动环境。使用当前受测源码的原始 Cargo Release 产物。两个受测版本都须具备下文的启动隔离边界。
- 为每次比较独占采样设备和显示器。Baseline 和 candidate 都用相同 Release 构建参数、同一终端网格、相同字体/DPI/刷新率和同一自动化输入方法。每次运行使用独立 build 和新的配置目录，交替运行 baseline/candidate。暂停其他编译、测试和 GPU 工作。
- 使用 Node.js 22 或更新版。Windows 需要可用的交互式托盘、原生 Git for Windows `bash.exe`（使用实际 `usr/bin/bash.exe`，非 WSL）和已按开发规范准备及暂存的 ConPTY 资产。macOS 使用 `/bin/bash` 及实际可用的托盘。

在单独的控制终端运行以下命令。`<commit>` 替换为两个二进制的实际完整提交。按当前测量计划设置 `--cycles`，初次验收用 `1`。`--steady-ms` 是核对完成后等候采样的毫秒数。

```powershell
pwsh scripts/cargo-windows.ps1 build --release --locked -p lyrel -p lyrel-cli
node scripts/bench/desktop-restore-434.mjs --app target/release/lyrel-app.exe --cli target/release/lyrel.exe `
  --bash 'C:/Program Files/Git/usr/bin/bash.exe' --revision '<commit>' --cycles 1
```

```sh
cargo build --release --locked -p lyrel -p lyrel-cli
node scripts/bench/desktop-restore-434.mjs --app target/release/lyrel-app --cli target/release/lyrel \
  --revision '<commit>' --cycles 1
```

窗口启动后按控制终端提示操作。使用本次启动的窗口和该窗口自带的关闭按钮。macOS 也可以用 `⌘⇧W`。每次启动、重附着或唤醒后，直接向恢复时的焦点键入一次 `b434 TOKEN` 并按 Enter。先点窗格会绕过焦点验证。左、右和备用窗格的标记分别由对应 Shell 打印。检查画面时可滚动访问旧标记，检查后回到实时提示符。保留脚本输出目录和每次画面记录。

| 阶段 | 预期身份与行为 |
| --- | --- |
| `warm` | 关闭最后一个窗口并退出 GUI，专用 daemon 及三个 Shell 继续运行。路径为空的再次启动产生新 GUI PID，恢复原 pane ID、Shell PID 和标记。 |
| `cold` | 关闭 GUI 后正常停止专用 daemon，确认三个旧 Shell 都退出。路径为空的再次启动从已保存布局和屏幕恢复。预期 daemon PID、pane ID、Shell PID 更新，原工作区、标签身份、顺序、比例、目录与标记保留。这里的冷启动指 GUI 和 daemon 均未运行，保留操作系统文件缓存。 |
| `wake` | `F8` 触发产品的 `HibernateTab`，切到 `Anchor434`，旧左右 Shell 停止，布局与旧 ID 保留为休眠记录。选择 `Restore434` 唤醒，恢复左右窗格及屏幕，两个 Shell 重新创建，备用窗格保持原会话。这个动作只针对应用内标签页。 |
| `reattach` | 托盘保留图形进程。关闭窗口后验证工作区失去附着且 Shell 保留。路径为空的启动将请求转交同一 GUI PID，恢复原 pane ID、Shell PID 和标记。当前平台缺少可用托盘时该阶段缺前提。 |

脚本记录 `events.jsonl`（启动、工作区树、daemon 和 Shell PID、各阶段的稳态检查点和错误）、`shell.tsv`（实际 Shell 收到的 token、pane、workspace、cwd）、每阶段的 CLI 回放和 `gui.log`。标记恰好在受测窗格出现一次，其他窗格的 CLI 回放不含本次回执。窗口画面另留证。结束时回收脚本启动的 GUI 进程和本次专用 daemon，保留证据目录。若需手动收尾，先确认目录和 PID 属于当前样本。用本轮测试 CLI 创建子进程，在其环境中清除继承的 `LYREL_*`，将 `LYREL_CONFIG_DIR` 设为本轮的 `config` 目录后执行 `server stop`。产品规则见 [会话恢复、休眠与关闭窗口](../../docs/window/tabs-and-splits.mdx#会话恢复休眠与关闭窗口)。

## Windows 首字符缺失复现

历史样本用 `window.type()` 输入整串，再用 `window.press('enter')` 提交。暖启动失败的那一轮先从 CLI 中找到存活窗格的 `[left]` 回放，再等 AX 树出现 `Restore434`。休眠失败的那一轮在 `F8` 后等 AX 树只剩一个 `textfield`。两个预期 Shell 都存活，CLI 回放分别显示 `434 warm1` 和 `434 hold1`，紧跟 `bash: 434: command not found`。另一次暖启动用 AX 标签存在作为发送条件，整串输入成功。唤醒复测先单独发 `b`，从左窗格的 CLI 回放读到 `[left] $ b` 后接续命令并收到回执。存活与回放确认 Shell 状态，AX 标签和输入面数量确认窗口内容。两次失败都缺少当时的终端焦点和窗口收键数据。

用已经构建的隔离产物运行下列命令。`warm` 只循环暖启动。将它替换为 `wake` 可循环 `F8` 切到备用窗格和 `Alt+1` 唤醒，分别提交 `holdN` 和 `wakeN`。每个回执仍按原场景校验。等待回执失败时，脚本会在关闭窗口和停止 daemon 前保存该工作区所有存活窗格的回放，并在 `events.jsonl` 的 `input-failure` 事件中记录 token、预期窗格和文件名。

```powershell
node scripts/bench/desktop-restore-434.mjs --app target/release/lyrel-app.exe --cli target/release/lyrel.exe `
  --bash 'C:/Program Files/Git/usr/bin/bash.exe' --revision '<commit>' --input-check warm --cycles 5 --steady-ms 0
```

1. 复用历史样本的发送条件，同一次尝试只提交一次。暖启动时用本轮 CLI `pane ls --all` 找存活窗格，并用 `capture %PANE --plain --scrollback` 检查回放末尾的 `[left]` 后再等 AX 标签。用 x64 Spy++ 按 `launch` 事件中的 GUI PID 找到本轮窗口，只记录该窗口的焦点和键盘消息，包括 `WM_SETFOCUS`、`WM_KILLFOCUS`、`WM_KEYDOWN`、`WM_CHAR` 和 `WM_KEYUP`。保留实际发送方式、修饰键状态、首字符及后续字符的窗口消息顺序，以及同一 token 的 `shell.tsv` 或失败回放。如果挂接消息记录耗时使发送延后，单独标记该样本。回放驱动若通过其他接口送入文本，也要在该接口观察交付数据，才能判定字符缺失。
2. 用相同的发送方式另跑一轮，等画面显示目标窗格的提示符和焦点样式，且 AX 显示焦点在终端输入面，再发整串。再用这一就绪条件做单键对照，先用 `window.press('b')`，通过 `lyrel --json capture %PANE --plain --scrollback` 确认目标窗格出现未提交的 `$ b`，然后接续其余字符并提交。CLI 子进程使用本轮 `LYREL_CONFIG_DIR`。两轮都沿用窗口恢复后的焦点。
3. 若随后字符到达受测窗口，但 `b` 对应的同类有效消息缺失或去了其他 HWND，先检查回放驱动和窗口激活。若与随后字符同类的有效 `b` 消息已到同一 HWND，随后字符也已落到预期 Shell，但所有窗格回放均缺 `b`，沿窗口内输入链定位。用能匹配当前构建的符号记录 `TerminalView::on_key_down`、`TerminalInputHandler::replace_text_in_range` 的事件和 pane ID，再检查 pane 写入的字节。若 `b` 出现在其他窗格，保留两个 pane ID，检查焦点交接。若差异只在早发时出现，按窗口焦点、终端输入面注册、Shell 回显的顺序定位。若原样本未复现，保留样本数和门槛，归因仍待原生证据。

## 启动隔离边界

- GUI 通过 `--config-dir` 启动和派生 daemon，CLI 通过子进程的 `LYREL_CONFIG_DIR` 定位本轮实例。实例目录持有 IPC、锁、布局、历史、日志和更新状态。旧机器树的迁移也只会看到本轮的 `HOME`／`XDG_DATA_HOME` 或 `LOCALAPPDATA`。见 [`spawn_detached`](../../crates/lyrel-protocol/src/daemon/spawn.rs#L1340) 和 [`adopt_legacy_data_dir`](../../crates/lyrel-protocol/src/core/machine/paths.rs#L85)。
- Release 保留启动时的 hook 检查。它只刷新已发现的过期 hook。空的 `agent_launch` 与隔离的 `HOME`／`USERPROFILE`、`LOCALAPPDATA`、`XDG_CONFIG_HOME` 使 Pi、OMP、Kimi、Hermes、OpenCode 和 Antigravity 只探查新建目录。Windows 重新读取注册表中的 pane 环境时，本轮配置中的隔离目录优先。真实 Bash 使用夹具的 rc 与 Readline 配置，`-p` 阻止导入环境中的 Shell 函数和启动脚本。见 [`refresh_hooks_at_launch`](../../crates/lyrel-protocol/src/core/agent_hooks.rs#L824)、[`agent_home`](../../crates/lyrel-protocol/src/core/agent_hooks/launch_scope.rs#L76) 和 [`merge_environment`](../../crates/lyrel-core/src/daemon/windows_env.rs#L85)。
- Windows 的 AUMID 是进程级标记。原始 Cargo 产物走跳过 Start Menu 快捷方式写入的分支。Explorer 菜单标签只允许真实 Windows Roaming AppData 下的默认实例写入，独立的 `--config-dir` 由此跳过 `HKCU` 写入。CLI 安装和更新检查在配置中禁用。更新启动清理仍会运行。脚本拒绝安装标记和 `.app` 内的产物，并将 Windows 交互用户进程的临时目录指向本轮目录。见 [`decide_with`](../../crates/lyrel-terminal/src/aumid.rs#L167)、[`refresh_labels`](../../src/core/explorer_context_menu.rs#L362) 和 [`stage_roots`](../../src/core/update.rs#L1052)。
- macOS GUI 会在可见窗口前探测账户数据库中的登录 Shell 来补充 `PATH`。脚本只接受系统 `/bin/zsh` 或 `/bin/bash`，并为子进程设置隔离的 `HOME` 和 `ZDOTDIR`。系统级启动脚本仍会运行，包括 zsh 的 `/etc/zshenv` 或 Bash 的 `/etc/profile`。如有自定义脚本，请先核对其写入目标。若它会写入实际用户配置或无法核实，请在独立的原生 macOS 环境运行。见 [`enrich_path_from_login_shell`](../../src/lib.rs#L511) 和 [`login_shell`](../../crates/lyrel-protocol/src/core/shells.rs#L242)。

## 采样与判读

- `accepted.observedMs` 从启动调用前计时至读取 Shell 回执。`wake` 的起点是操作者在控制终端按下 Enter 后、发送桌面快捷键前的时刻。人工回放包含操作员切换、反应、键入，以及 20 ms 间隔的检查，适合复现路径与查错。用于性能结论的样本应使用相同的原生输入回放，并另记实际启动或选择键、输入与正确回执帧的原生时钟。按路径单列样本数、p50/p95、分布范围、超过两个刷新周期的比例、超时和无法关联的样本。
- Windows 正确帧的归因使用 #430 的 `scripts/terminal-display-analysis.mjs` 与 [终端基准说明](README.md)。PresentMon 限定本次 GUI PID，使用同源 QPC 时间。`inputs.csv` 记录实际启动或标签选择事件及样本 ID，`frames.csv` 中相同 ID 要来自隔离窗口的像素观测，具体识别恢复后的左窗格标记和当前 token 回执。保留首个正确恢复帧与首个正确回执帧的独立记录。相对视频 PTS 或 `events.jsonl` 的 Node 单调时钟只有验证同源或校准后才能与 QPC CSV 联用。macOS 需要原生显示采集与时间相关证据，单列没有可用关联的动作。
- 在控制终端外启动系统采集，覆盖启动前到最后一轮。Windows 用 WPR 记录进程生命周期和 CPU，采样 `Get-Process -Id <本次PID>` 的 `CPU`、`WorkingSet64`、`PrivateMemorySize64` 和 `PeakWorkingSet64`。用已发现的本地化 GPU Engine `Utilization Percentage` 计数器按本次 GUI PID 筛选，并记录可用引擎及采样间隔。macOS 用 Instruments 记录 App/daemon 生命周期和 CPU，按本次 PID 采样 `ps -o pid=,rss=,time= -p <本次PID>`，以 KiB 记录 RSS。用原生工具记录可归因的 GPU 样本。将启动峰值和带 `steady` 事件的附着、脱离、休眠稳态分开，分别归因 GUI、daemon、Shell 和平台子进程，例如 ConPTY 宿主。CPU 约定用 CPU 秒及相对墙钟的逻辑核数。缺失的进程/GPU/显示字段标为不可用。
- 保存实际 GPU 和驱动、刷新率、分辨率、缩放、网格、原始 trace、复现命令、哈希、版本、每轮证据和采样跨度。每条路径按交替运行的分布比较等价工作量，联查焦点、回放和进程寿命。只有超过基线波动且可见行为等价的变化才作为改进证据。没有原生平台测量时保留为未验证，没有比较数据时不作优化结论。
