# #434 桌面会话恢复

在 Windows x64 与 macOS 原生桌面运行 `desktop-restore-434.mjs`。场景从一个新建的 `Workspace434` 开始。`Restore434` 有左右两个平铺窗格，`Anchor434` 有一个始终醒着的窗格。三个 Bash 会话分别进入 `left`、`right`、`anchor` 目录，并用键盘输入唯一命令。CLI 只核对会话和终端回放、命名夹具以及停止专用 daemon。受测输入完整经过桌面窗口、终端和真实 Shell。

## 前提与运行

- 使用能访问原生桌面的专用测试账户，且该账户没有已有的 Lyrel 安装会话或代理配置。`--disposable-account` 是对此前提的确认。Release 启动会按用户账户刷新过期的 Lyrel 代理钩子，`--config-dir` 不隔离该行为。测试账户使用自己的环境，也不要从工作账户继承代理的配置路径。Windows 的 AUMID 是进程级标记。使用 Cargo 构建目录中的可执行文件以避免便携版的 Start Menu 快捷方式接线。
- 为每次比较独占采样设备和显示器。Baseline 和 candidate 都用相同 Release 构建参数、同一终端网格、相同字体/DPI/刷新率和同一自动化输入方法。每次运行使用独立 build 和新的配置目录，交替运行 baseline/candidate。暂停其他编译、测试和 GPU 工作。
- 使用 Node.js 22 或更新版。Windows 需要可用的交互式托盘、原生 Git for Windows `bash.exe`（使用实际 `usr/bin/bash.exe`，非 WSL）和已按开发规范准备及暂存的 ConPTY 资产。macOS 使用 `/bin/bash` 及实际可用的托盘。

在单独的控制终端运行以下命令。`<commit>` 替换为两个二进制的实际完整提交。按当前测量计划设置 `--cycles`，初次验收用 `1`。`--steady-ms` 是核对完成后等候采样的毫秒数。

```powershell
pwsh scripts/cargo-windows.ps1 build --release --locked -p lyrel -p lyrel-cli
node scripts/bench/desktop-restore-434.mjs --app target/release/lyrel-app.exe --cli target/release/lyrel.exe `
  --bash 'C:/Program Files/Git/usr/bin/bash.exe' --revision '<commit>' --disposable-account --cycles 1
```

```sh
cargo build --release --locked -p lyrel -p lyrel-cli
node scripts/bench/desktop-restore-434.mjs --app target/release/lyrel-app --cli target/release/lyrel \
  --revision '<commit>' --disposable-account --cycles 1
```

窗口启动后按控制终端提示操作。使用本次启动的窗口和该窗口自带的关闭按钮。macOS 也可以用 `⌘⇧W`。每次启动、重附着或唤醒后，直接向恢复时的焦点键入一次 `b434 TOKEN` 并按 Enter。先点窗格会绕过焦点验证。左、右和备用窗格的标记分别由对应 Shell 打印。检查画面时可滚动访问旧标记，检查后回到实时提示符。保留脚本输出目录和每次画面记录。

| 阶段 | 预期身份与行为 |
| --- | --- |
| `warm` | 关闭最后一个窗口并退出 GUI，专用 daemon 及三个 Shell 继续运行。路径为空的再次启动产生新 GUI PID，恢复原 pane ID、Shell PID 和标记。 |
| `cold` | 关闭 GUI 后正常停止专用 daemon，确认三个旧 Shell 都退出。路径为空的再次启动从已保存布局和屏幕恢复。预期 daemon PID、pane ID、Shell PID 更新，原工作区、标签身份、顺序、比例、目录与标记保留。这里的冷启动指 GUI 和 daemon 均未运行，保留操作系统文件缓存。 |
| `wake` | `F8` 触发产品的 `HibernateTab`，切到 `Anchor434`，旧左右 Shell 停止，布局与旧 ID 保留为休眠记录。选择 `Restore434` 唤醒，恢复左右窗格及屏幕，两个 Shell 重新创建，备用窗格保持原会话。这个动作只针对应用内标签页。 |
| `reattach` | 托盘保留图形进程。关闭窗口后验证工作区失去附着且 Shell 保留。路径为空的启动将请求转交同一 GUI PID，恢复原 pane ID、Shell PID 和标记。当前平台缺少可用托盘时该阶段缺前提。 |

脚本记录 `events.jsonl`（启动、工作区树、daemon 和 Shell PID、各阶段的稳态检查点和错误）、`shell.tsv`（实际 Shell 收到的 token、pane、workspace、cwd）、每阶段的 CLI 回放和 `gui.log`。标记恰好在受测窗格出现一次，其他窗格的 CLI 回放不含本次回执。窗口画面另留证。结束时回收脚本启动的 GUI 进程和本次专用 daemon，保留证据目录。若需手动收尾，先确认目录和 PID 属于当前样本，再给测试 CLI 的子进程设置该目录的 `LYREL_CONFIG_DIR` 并执行 `server stop`。产品规则见 [会话恢复、休眠与关闭窗口](../../docs/window/tabs-and-splits.mdx#会话恢复休眠与关闭窗口)。

## 采样与判读

- `accepted.observedMs` 从启动调用前计时至读取 Shell 回执。`wake` 的起点是操作者在控制终端按下 Enter 后、发送桌面快捷键前的时刻。人工回放包含操作员切换、反应、键入，以及 20 ms 间隔的检查，适合复现路径与查错。用于性能结论的样本应使用相同的原生输入回放，并另记实际启动或选择键、输入与正确回执帧的原生时钟。按路径单列样本数、p50/p95、分布范围、超过两个刷新周期的比例、超时和无法关联的样本。
- Windows 正确帧的归因使用 #430 的 `scripts/terminal-display-analysis.mjs` 与 [终端基准说明](README.md)。PresentMon 限定本次 GUI PID，使用同源 QPC 时间。`inputs.csv` 记录实际启动或标签选择事件及样本 ID，`frames.csv` 中相同 ID 要来自隔离窗口的像素观测，具体识别恢复后的左窗格标记和当前 token 回执。保留首个正确恢复帧与首个正确回执帧的独立记录。相对视频 PTS 或 `events.jsonl` 的 Node 单调时钟只有验证同源或校准后才能与 QPC CSV 联用。macOS 需要原生显示采集与时间相关证据，单列没有可用关联的动作。
- 在控制终端外启动系统采集，覆盖启动前到最后一轮。Windows 用 WPR 记录进程生命周期和 CPU，采样 `Get-Process -Id <本次PID>` 的 `CPU`、`WorkingSet64`、`PrivateMemorySize64` 和 `PeakWorkingSet64`。用已发现的本地化 GPU Engine `Utilization Percentage` 计数器按本次 GUI PID 筛选，并记录可用引擎及采样间隔。macOS 用 Instruments 记录 App/daemon 生命周期和 CPU，按本次 PID 采样 `ps -o pid=,rss=,time= -p <本次PID>`，以 KiB 记录 RSS。用原生工具记录可归因的 GPU 样本。将启动峰值和带 `steady` 事件的附着、脱离、休眠稳态分开，分别归因 GUI、daemon、Shell 和平台子进程，例如 ConPTY 宿主。CPU 约定用 CPU 秒及相对墙钟的逻辑核数。缺失的进程/GPU/显示字段标为不可用。
- 保存实际 GPU 和驱动、刷新率、分辨率、缩放、网格、原始 trace、复现命令、哈希、版本、每轮证据和采样跨度。每条路径按交替运行的分布比较等价工作量，联查焦点、回放和进程寿命。只有超过基线波动且可见行为等价的变化才作为改进证据。没有原生平台测量时保留为未验证，没有比较数据时不作优化结论。
