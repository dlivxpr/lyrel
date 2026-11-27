# 工作台性能覆盖表（#429）

本表用工作负载和正确结果索引 [#429](https://github.com/dlivxpr/lyrel/issues/429) 的验收证据。产品范围取自 [`docs.json`](../../docs/docs.json) 和[架构中的 14 个包](../../docs/reference/architecture.mdx)。同一行中的平台和路由分别采样，各自记录结果。共享库通过实际调用它们的程序计量。

## 测量记录

每次正式采样独占设备，同机交替运行基线和候选 Release。远程实验同时保留两端，记录 RTT 和固定生产速率。保存两个提交、构建命令、工具版本、OS／硬件、窗口／网格尺寸、显示器和实际刷新率、fixture 和重放命令、预热、迭代次数、采样时间段及关闭后的等待时间。至少保留每次运行的原始样本和变化范围。两个显示器的刷新率可能不同。

每个可见动作记录输入至首次正确显示的 p50／p95、超过两个刷新周期的样本数、未匹配数和总数。有序输出、被选中的路径、接收输入的 pane、保存的文件字节等都由独立的目标数据校验。窗口截图可验证内容。时序结论另需可关联的显示事件。后台工作负载同时运行前台回显探针。

各进程分别记录启动、峰值、稳态和多轮释放后的内存，以及相同工作量的 CPU 时间。记录显存或 GPU 引擎可用性、采样分辨率和帧间隔。Windows 每阶段可用 `sample-windows-resources.ps1` 返回的 PowerShell 对象保存 CSV。`guiPid`、`daemonPid` 取自本轮隔离实例的进程句柄。示例的 `visible` 阶段须覆盖一个完整固定速率的输出周期。

```powershell
& ./scripts/bench/sample-windows-resources.ps1 -Processes @{ gui = $guiPid; daemon = $daemonPid } -Phase visible -Seconds 30 |
    Export-Csv -NoTypeInformation -NoClobber -Path (Join-Path $runDir 'visible.csv')
```

`qpcMs` 是本机 QPC 毫秒。`cpuMs` 是进程累计 CPU 毫秒，`privateBytes` 与 `workingSetBytes` 使用字节。计算相邻边界的 `ΔcpuMs / ΔqpcMs` 得逻辑核数。对完整包住 N 行的阶段，`ΔcpuMs × 10000 / N` 得每万行 CPU 毫秒。GPU 取对应 PID 各引擎的最大利用率，系统计数器自行刷新。`no-instance` 或 `counter-unavailable` 都按缺失统计。采样峰值取实际读数并注明间隔。子进程可能短于采样周期，另从子进程生命周期记录 CPU／峰值或注明缺样。

## 工作负载与证据归属

下表索引每项的运行入口和可核对的结果。初始基线为 `cb897016`。当前各行的正式 Release 延迟、内存及 CPU/GPU 对照均待采样，处置保持开放。工单中的 50,000 路径排序耗时、历史容量和 Debug 终端数据保留为实验线索。每项结果按 `平台/路由、revision、重放命令、通过的正确性断言、延迟、内存、CPU/GPU、变化范围、处置、原始记录` 追加。纯 headless 任务的显示/GPU 记为不适用，其余缺项给出具体前提。

| 工单 | 工作负载及通过条件 | 执行路径 | 单独计量的进程 |
|---|---|---|---|
| [430](https://github.com/dlivxpr/lyrel/issues/430) | 空闲／可见／隐藏输出；9,600 行 806,400 字节完整有序，键与 IME commit/cancel 正确，返回看到末尾 | Windows、macOS 本地 PTY | GUI、daemon、producer |
| [431](https://github.com/dlivxpr/lyrel/issues/431) | 忙时滚至编号行，搜索命中／未命中，选区和复制字节正确 | Windows、macOS 本地 PTY | GUI、daemon、producer |
| [432](https://github.com/dlivxpr/lyrel/issues/432) | 标签／分屏／工作区切换和 resize，首个正确画面与下一键落在同一目标 | Windows、macOS | GUI、daemon、各目标 shell |
| [433](https://github.com/dlivxpr/lyrel/issues/433) | Ctrl-C 获远端任务确认，校验中断前有序输出、恢复后会话身份与提示符 | Windows、macOS → Linux；原生 SSH 和 server 工作区各跑一次 | GUI、本地 daemon、远端 daemon／任务 |
| [434](https://github.com/dlivxpr/lyrel/issues/434) | 冷暖启动、窗口分离／重附、标签休眠／唤醒；核对 pane 树、cwd、标记及首个有效输入 | Windows、macOS | GUI、daemon、shell |
| [435](https://github.com/dlivxpr/lyrel/issues/435) | 固定终端图形／字形的首次显示、隐藏回放、多轮释放；像素位置与尺寸正确 | Windows、macOS；本地及支持的远端 pane | GUI、daemon、producer |
| [436](https://github.com/dlivxpr/lyrel/issues/436) | 空闲／忙时 capture、exec，回放到 live 有序，stdout／退出状态与下一条命令正确 | Linux headless，Windows、macOS CLI；另测 GUI 并发订阅 | CLI、server／daemon、shell、GUI |
| [437](https://github.com/dlivxpr/lyrel/issues/437) | Host 文件列举／监视、SFTP 传输、端口转发；核对文件字节与转发 payload | 原生 SSH、server 工作区、适用 WSL 路由 | GUI、daemon、remote server、传输端点 |
| [438](https://github.com/dlivxpr/lyrel/issues/438) | 5k／50k 文件、Unicode／同分、快速增删；全局搜索及终端输入框打开所选路径 | Windows、macOS，本机及所属 Host | GUI、Host 所在进程 |
| [439](https://github.com/dlivxpr/lyrel/issues/439) | 文件树、监视、Git 状态、worktree 切换；选中路径与真实仓库一致 | Windows、macOS，本机／远程 | GUI、Host、Git |
| [440](https://github.com/dlivxpr/lyrel/issues/440) | 大文本及结构化 diff，快速切换后核对 revision 和选中 hunk | Windows、macOS，本机／远程 | GUI（含 content-diff）、Host、Git |
| [441](https://github.com/dlivxpr/lyrel/issues/441) | 1 MiB 文档 100 次独立修改及连续输入对照，Undo／Redo、选择、编码、行尾与 IME | Windows、macOS 原生 editor | GUI（含 Markdown editor、codec）、文件 Host |
| [442](https://github.com/dlivxpr/lyrel/issues/442) | 源码打开、编辑、保存与关闭；受控 LSP 迟到结果保持当前文档与 revision | Windows、macOS | GUI、语言服务器、Host |
| [443](https://github.com/dlivxpr/lyrel/issues/443) | 结构树／表格、源位置与修订切换，保存后核对文件；只读 SQLite 使用版本一致的分页结果 | Windows、macOS，本机／支持的远程 Host | GUI、文档引擎所在进程、SQLite 所属 Host |
| [444](https://github.com/dlivxpr/lyrel/issues/444) | 固定图像、PDF／EPUB、draw.io 和 HTML，核对首次可读页、尺寸及关闭后的资源 | Windows WebView2、macOS WKWebView、原生渲染器 | GUI、WebView renderer、Host |
| [445](https://github.com/dlivxpr/lyrel/issues/445) | agent 状态、会话发现、编排、通知返回；事件和输入属于预期会话 | Windows、macOS 桌面、Linux headless CLI 支持的操作 | GUI、CLI、daemon、agent |
| [446](https://github.com/dlivxpr/lyrel/issues/446) | skills、usage、主机指标、GitHub；受控响应校验工作区和展示数据 | Windows、macOS，本机／远程 Host | GUI（含 insights）、Host、服务耗时 |
| [447](https://github.com/dlivxpr/lyrel/issues/447) | 字体、主题、按键、shell integration、粘贴、链接、菜单／托盘／通知，核对目标、值和载荷 | Windows、macOS 系统集成 | GUI、daemon、shell、平台组件 |
| [448](https://github.com/dlivxpr/lyrel/issues/448) | 一次性安装、已校验更新、故障恢复与重新启动；核对版本、会话及可用 shell | Windows 安装／便携、macOS app 支持的交付路径 | 安装器、updater、GUI、daemon、shell |
| [449](https://github.com/dlivxpr/lyrel/issues/449) | 在集成版本重跑前台输入、滚动、切换、远端中断和代表性后台负载；复核各行处置 | 全部适用平台和路由 | 分别计量各行所列进程 |

只有正确结果和同等质量成立、收益超过重复基线波动、较高优先级指标仍在波动范围内，才记录为改善。基线表现足够的行保留支持该判断的原始运行。发生实测取舍时链接维护者决定。缺平台、缺关联帧和未解决的性能问题继续列在对应开放工单。

## 首轮运行记录

Windows x64 基线 `cb897016`，Core Ultra 7 265KF（20 个逻辑处理器）、RTX 5080，隔离 Release 工作台。窗口采集尺寸 1440 × 900，Hack 16，输出每秒 400 行。以下是单轮正确性回放期间的 WGC 合成时间，构建任务与本次回放有重叠。正式基线与候选需在独占时段交替采样。

| #430 前台输入所在场景 | 正确标记／输入 | WGC p50 | WGC p95 | 正确性证据 |
|---|---:|---:|---:|---|
| 空闲 | 6/6 | 19.52 ms | 22.60 ms | CLI 回放与逐键标记一致 |
| 本窗格持续输出 | 80/80 | 19.69 ms | 28.30 ms | 9,600 行按序，最后一批在 23,960.67 ms 完成 |
| 隐藏窗格持续输出 | 80/80 | 19.26 ms | 27.01 ms | 返回后可见 `PLAIN 9600`、`PLAIN_DONE`，随后 `a` 只到达该窗格 |

系统中文 IME 提交 `中文` 后，屏幕与 PTY 各出现一次 `IME 中` 和 `IME 文`。Escape 取消另一轮预编辑后，PTY 保留原文。PresentMon 建立 ETW 会话返回 `access denied`（退出码 6），三组合计 166 个输入的显示事件均未匹配。重放路径见[回显探针](terminal-echo.md)。

同机候选 Release 的 `lyrel-app.exe` SHA-256 为 `6455b0ee3bbeb5c4cce08d71ab3deddf24cac2aeed4609f413b8b1419a4af35b`。构建期间集成分支有新提交，因此以实测二进制摘要标识候选。每个场景取一轮，空闲取 6 键，两种负载各取 80 键，键间隔均为 250 ms。

| 候选前台输入场景 | 正确字形／输入 | WGC p50 | WGC p95 | 生产者最后一批完成时间 |
|---|---:|---:|---:|---:|
| 空闲 | 6/6 | 19.08 ms | 28.65 ms | 不适用 |
| 本窗格持续输出 | 80/80 | 22.03 ms | 27.54 ms | 23,960.31 ms |
| 隐藏窗格持续输出 | 80/80 | 19.71 ms | 27.33 ms | 23,960.17 ms |

两组负载的 PTY 回放均通过 9,600 行顺序和内容校验。切回隐藏窗格时，最后一屏显示 `PLAIN 9600` 和 `PLAIN_DONE`。随后输入 `a`，该窗格收到一次回显，原前台仍为 80 次。另两次空闲采集未纳入分布，分别缺少与输入匹配的字形、复用了序号已递增的窗格。候选未取得 PresentMon 事件，166 个输入的物理显示时延均未配对。缺少交替重复测量，尚不能判断两版差异。

两组负载各采约 30 秒进程计数。可见／隐藏两轮的 GUI 累计 CPU 时间分别为 2,562.50／703.13 ms，daemon 为 1,453.13／1,718.75 ms，生产者为 203.13／156.25 ms。约一秒间隔采样到的 GUI 私有内存峰值分别为 120.57／118.95 MiB，daemon 为 26.40／28.87 MiB，生产者为 3.37／3.46 MiB。两轮 GUI 最忙 GPU 引擎的采样中位数均为 0%，另两个进程无可用引擎样本。

#433 在 Windows x64 经回环 SSH 连接 Linux 容器，两种路由各预热一次、测量两次。原生 SSH 的 `Ctrl-C` 到远端回执为 1.3943–1.3948 ms，server 工作区为 1.4846–1.6087 ms。两条路由均收到 `SIGINT` 回执和退出码 130，恢复后校验了 3,072 行并向原 shell 成功输入命令。原生路由分离后重新附着耗时 14.83 ms，server 工作区重启本机 router 后恢复到可用 shell 耗时 789.94 ms，各测一次。观察边界是 pane 协议，跨物理主机和桌面显示延迟待测。

Windows x64 的 [#434 恢复回放](desktop-restore-434.md) 使用三个 Git Bash 窗格。暖启动保留了三个 shell PID 和原有标记。冷启动恢复工作区、两个标签、分屏比例和历史，重建三个 shell。两次恢复后的第一条命令均由左窗格接收。独立的托盘重附着试验中，工作区先解除附着，再由原 GUI 接受重开请求，daemon 和三个 shell 的 PID 均一致，左窗格显示 `R434:trayround1:left`。

另两次整串键盘回放分别在休眠切换和暖启动后仅收到 `434 hold1` 和 `434 warm1`，首字符 `b` 缺失。独立复测时，唤醒与休眠切换后的目标 shell 各收到了完整命令，逐键发送的 `b` 也出现在唤醒后的左窗格。两次失败的原因与发生频率待查。上述观察来自正确性试验，恢复延迟分布及进程资源仍待采样。

Linux headless 上同一 `cli_e2e --capture-exec-workload` 驱动验证了 #436 的四种新旧 CLI／daemon 组合，基线为 `cb897016`，候选实现为 `70a46f74`。每种组合均取得 `verified`，包括忙时拒绝、退出码、输出顺序和后续命令。首次正确性运行中，空闲 `capture` 从 334.78 ms 至 20.02 ms，空闲 `exec` 完成从 361.24 ms 至 29.57 ms。两次各一个样本，正式轮间波动与资源对照待测。命令见[CLI 工作负载](README.md#cli-capture-与-exec436)。

Windows x64 上的 Git Bash 回放分别使用基线 `cb897016` 和集成版 `038fa60c` 的 CLI 与 server。两轮均通过 2,400 行有序回放、忙时捕获与拒绝执行、命令退出码和后续输入校验。基线走兼容路径，候选协商 `ReplayEnd`。空闲 `capture` 分别为 289.78 ms 和 47.02 ms，空闲 `exec` 分别为 1,049.31 ms 和 1,339.75 ms。每版各一次，`exec` 在这轮候选上较慢，重复采样后再判断差异。

随后在同一台 Windows 工作站交替运行三对无桌面订阅的 Release 回放。六轮均取得 `verified`。下表是每版三轮的最小值至最大值，派发耗时从 CLI 启动量到 Git Bash 中的生产者写入启动标记。

| 无桌面订阅，三轮 | 基线 `cb897016` | 候选 `038fa60c` |
|---|---:|---:|
| 空闲 `capture` 完成 | 192.23–206.18 ms | 48.18–50.10 ms |
| 持续输出时 `capture` 完成 | 59.94–61.83 ms | 47.13–49.96 ms |
| 空闲 `exec` 派发 | 671.93–732.53 ms | 819.45–831.51 ms |
| 空闲 `exec` 完成 | 742.20–836.14 ms | 888.36–892.35 ms |

桌面同时订阅时，两版各一轮均通过输出顺序、退出状态和会话存续校验。空闲 `capture` 完成为 192.29／49.22 ms（基线／候选），空闲 `exec` 完成为 664.48／358.82 ms。两种混合组合各跑一轮也均通过。旧 CLI 配新 server 和新 CLI 配旧 server 的空闲 `capture` 分别为 206.53 和 193.51 ms，空闲 `exec` 分别为 842.07 和 838.13 ms。无桌面三轮中，候选 `capture` 较快，空闲 `exec` 较慢，差异在生产者启动时已出现。该阶段包含 shell 调度和子进程启动，原因待定位。

随后将种子命令自己的 `OSC 133` 提示符末尾标记作为空闲条件，在同机用同一个驱动交替测量三对。六轮中，种子 CLI 退出后仍需 792.01–809.77 ms 才能观察到完整提示符。六轮均通过有序输出、忙时拒绝、退出状态及后续输入校验。下表保留最小值至最大值。

| 等到完整提示符后，Windows 无桌面订阅 | 基线（ms） | 候选（ms） |
|---|---:|---:|
| 空闲 `capture` 完成 | 352.60–367.75 | 47.97–51.77 |
| 空闲 `exec` 派发至生产者启动 | 551.90–615.13 | 250.39–306.31 |
| 空闲 `exec` 完成 | 626.30–720.30 | 322.18–367.13 |

三对中，候选版在上述三个区间均较快。旧试验包含了前一条命令返回后未完成的提示符准备，保留作连续操作的数据。实测基线 CLI／server 的 SHA-256 分别为 `89f472e98c73f3d4f35e237d0bf4a23aaf56546048787c7f639cb31f2d9a9e04`、`aa74931c51ab9e85db539e94e14793e1d09214d9f759554204742d119c36031b`，候选分别为 `c0b29595310db551d0e3262c585e9a2e65f7f7e5dfb23c130318f863d063136d`、`2b9daee31a66aa35d01329e72082cbc130ca3c16dbcb053491aa5c805452adc3`。

#438 的同一 50,000 路径热索引上，5 个查询分别取 8 和 200 项，两版有序路径与分数的摘要全部一致。各条件连续运行 7 次。取 200 项时，宽查询 `f` 的中位数从 22.77 ms 至 0.76 ms，选择性查询 `file_04242.txt` 从 0.37 ms 至 0.45 ms。计时范围是排序调用。两个原生入口的按键至可选结果仍需分别采样。

Windows 上用 #438 的 5,000 文件夹具回放全局文件搜索。基线查询 `f` 时前八项符合夹具顺序。按 `Down`、`Enter` 打开第二项后，文档标签显示 `fritz.txt`，正文为 `LYREL_SEARCH_PATH=fritz.txt`。
同一索引上，停靠式消息框输入 `@f` 后显示相同的八项。按 `Down`、`Enter` 并输入 `END`，草稿为 `@fritz.txt END`。窗格回放保留了 `LYREL_SEARCH_READY`，误输入标记 `LYREL_SEARCH_UNEXPECTED_INPUT` 的出现次数为零。

在 50,000 文件的 Windows 候选工作区，全局搜索输入 `文` 得到 `f文-a.txt`、`f文-b.txt`。选择第二项后，正文为 `LYREL_SEARCH_PATH=f文-b.txt`。消息框从 `@frost` 快速退格、输入到 `@flarex` 再退格，记录的相邻按键间隔约 5 ms。最终菜单只显示 `flare-a.txt` 和 `flare-b.txt`，首项高亮。按 `Down`、`Up`、`Down`、`Enter` 并输入 `END` 得到 `@flare-b.txt END`，窗格回放中误输入标记出现零次。

Windows 候选 Release 实窗按 [#441 长文档回放](README.md#441-长文档编辑回放windowsmacos) 编辑两个 1 MiB 文件。在 `typing.md` 连续输入 100 个 `x`，保存、一次撤销、一次重做后，文件分别与 `typing-100.bin`、`original.bin`、`typing-100.bin` 逐字节一致。在 `replace.md` 对 100 行分别替换首字符，各撤销和重做 100 次。两个方向在第 50 次和第 100 次的保存结果均与对应修订逐字节一致，保留 UTF-8 BOM、混合行尾和文件末尾字节。

用系统中文输入法把首行选中的 `🎉` 替换成 `你好`，保存文件与 `ime-commit.bin` 一致。一次撤销后重新选中 `🎉`，输入拼音并按 Escape 取消，保存结果与 `replace-100.bin` 一致。一次重做恢复了 `ime-commit.bin`。回放中的行列指示在按键处理后更新，批量撤销或重做要等到预期行列再保存。

同一轮在两个文档仍打开时，GUI 私有内存为 296.11 MiB。关闭两份文档，分别在约 2 秒、10 秒后取样，GUI 为 113.91、113.70 MiB，daemon 三次均为 15.20 MiB。此轮同时保留了两份文档及其历史。单事务成本、两版峰值和多轮关闭后的内存仍需对照。

另用两个全新实例，各打开一份相同的 `replace.md`，每完成 25 次独立替换就保存、逐字节核对并读取进程计数。两版的四个保存点均通过。下表是 Windows GUI 的私有内存，各一轮。候选 GUI 的文件摘要见上文 #430 记录。

| 采样点 | 基线 `cb897016`（MiB） | 候选（MiB） |
|---|---:|---:|
| 打开文档 | 234.27 | 181.26 |
| 已替换 25 行 | 361.12 | 287.89 |
| 已替换 50 行 | 356.07 | 293.75 |
| 已替换 75 行 | 377.32 | 226.40 |
| 已替换 100 行 | 421.51 | 259.75 |
| 关闭约 2 秒 | 113.86 | 113.84 |
| 关闭约 10 秒 | 113.87 | 113.27 |

相对各自打开文档时的读数，第 100 次替换后 GUI 私有内存分别增加 187.24 和 78.49 MiB。这个区间的 GUI 累计 CPU 时间分别为 68.25 和 68.86 秒。第 100 次替换后 daemon 私有内存分别为 8.35 和 15.30 MiB。进程计数包含布局、保存和分配器保留的内存。输入至显示的 p95、连续取样峰值和重复运行的波动仍待测。

Windows 候选 Release 实窗完成一轮 [#443 结构文档回放](structured-documents.md)。JSONC 展开 3,500 项的 `payload` 后显示索引 `0`。在 `probe.target` 选择第二个 `phase` 并定位源码，选区落在第 7 行的 `"phase": "TREE-1111"`。结构编辑、撤销和重做依次显示 `TREE-2222`、`TREE-1111`、`TREE-2222`。源码连续退格、输入 `9` 后返回结构树，第二个值显示 `TREE-2229`，第一个值仍为 `"first\u4e2d"`。

CSV 表格显示 50,005 条记录，选中 `R2 C3` 按 Enter 后，源码选区为 `CELL-1111`。用 F2 修改后撤销、重做，再从源码将末位改为 `9`，返回表格显示 `CELL-2229`。两个格式在保存前均通过 `verify original`。分别保存、关闭、从文件树重开后，JSONC 仍显示两个预期属性，CSV 仍显示探针值、含逗号字段及多行字段。`verify edited all` 逐字节核对了 JSONC 的 822,725 字节及 CSV 的 3,375,102 字节。本轮记录正确性。首次可用投影延迟、旧修订是否曾短暂显示、两版进程资源和轮间波动仍待采样。

#448 的 Windows 便携包预检使用已发布的 `v0.5.0`（`ee9a10c5`）和 `v0.5.1`（`e03f1fa4`）。两个 ZIP 的 SHA-256 分别为 `4919445f94f180fb38b3c24d64f98db4c50c33dfafa6d59b06e51619e50624bc` 和 `af51c02b51813d76845b7fd942fa44b76ddcdf51f7d868bdc4bbc0b9dcf6691a`，均与各自的 `checksums.txt` 及 GitHub 资产摘要一致。按[便携更新步骤](../../docs/reference/updates.mdx#用本地资产复测安装更新和恢复)运行旧包的 `verify-portable`，一次成功预检用时 581 ms 并写出 `portable-payload`。旧包与暂存包的 CLI 分别报告 `0.5.0` 和 `0.5.1`。仅将校验文件中新包的摘要改为 64 个零后，预检退出码为 1，另行创建的暂存目录为空。安装替换、重开后会话和 shell 输入另行验收。

随后分别从两份全新的 `0.5.0` 便携安装运行 `install-portable`，每份使用独立配置、daemon 和 `cmd.exe` 工作区。两份旧版窗格均先执行命令输出独立的一行 `LYREL448_PREOK`。成功样本解包旧版用时 776 ms，新包预检用时 557 ms。从启动安装 helper 到其退出为 2,321 ms，退出码为 0，outcome 为 `ok: true`。自动重开的 `0.5.1` 恢复了同一工作区、标签、目录和历史，重建了 daemon 和 shell。原生窗口输入命令后显示 `LYREL448_NEWOK`，CLI 回放同时含有新旧结果行。

另一份安装通过相同预检后，仅将安装阶段使用的预期摘要改为 64 个零。helper 在 1,506 ms 后以退出码 1 结束，outcome 为 `ok: false`，并列出了预期与实际 SHA-256。自动重开的 `0.5.0` 保留原 daemon、shell、窗格、标签、工作区及目录，输入命令后显示 `LYREL448_RECOVEROK`。两个样本各运行一次，helper 用时包含等待旧窗口退出；到首条新命令可见的延迟和进程资源仍待采样。
