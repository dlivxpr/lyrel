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
