<a id="terminal-benchmark-harness"></a>
# 终端基准测试工具集

用于对比 lyrel 与 Alacritty、Ghostty 和 Kitty 的可复现吞吐量、FPS 和内存基准测试，遵循
[moktavizen/terminal-benchmark](https://github.com/moktavizen/terminal-benchmark)
的方法。
仅支持 macOS（驱动脚本使用 `/Applications` 路径、BSD `ps` 和 `pkill`）。

跨平台工作台性能实验的执行路径、逐进程采样和工单索引见[工作台性能覆盖表](workbench-coverage.md)。

<a id="what-it-measures"></a>
## 测量内容

| 测试 | 方法 | 采集指标 |
| --- | --- | --- |
| **纯文本 IO** | 在终端内使用 `cat` 输出一个 11 MB 的文本文件，运行 5 次 | 每次运行的耗时，单位为 ms（越低越好） |
| **帧率** | [DOOM-fire-zig](https://github.com/const-void/DOOM-fire-zig)，运行 5 次，每次约 14 秒 | 每次运行的累计平均 fps（越高越好） |
| **内存** | 冷启动，使用默认 shell，空闲 6 秒 | RSS，单位为 KB（lyrel = GUI + 守护进程） |

方法来源还测量了**输入延迟**，但需要使用高速摄像机
（[is-it-snappy](https://github.com/chadaustin/is-it-snappy)）——
这里无法自动化，因此跳过。

<a id="usage"></a>
## 用法

```bash
scripts/bench/setup.sh                  # 仅需一次：准备语料和打过补丁的 DOOM-fire（必要时还会准备 zig 0.14）
cargo build --release                   # 构建待测的 lyrel

scripts/bench/run_one.sh lyrel io       # 必须先运行：记录参考网格
scripts/bench/run_one.sh alacritty io   # 自动匹配 lyrel 的网格
scripts/bench/run_one.sh ghostty io
scripts/bench/run_one.sh kitty io
scripts/bench/run_one.sh lyrel fire     # 火焰测试也使用相同顺序
scripts/bench/run_one.sh alacritty fire
scripts/bench/run_one.sh ghostty fire
scripts/bench/run_one.sh kitty fire
scripts/bench/mem.sh                    # 四个终端各运行 3 次
```

结果保存在 `.bench/results/` 中（可通过 `$LYREL_BENCH_DIR` 覆盖工作目录，
通过 `$LYREL_BIN` 覆盖 lyrel 二进制文件路径）。

**测试运行期间：不要在窗口中输入，也不要隐藏或完全遮挡窗口**。macOS 的窗口调度会影响 FPS，比较时须保持相同的窗口可见性与机器负载。

<a id="how-it-drives-each-terminal"></a>
## 如何驱动各个终端

每个终端都会打开一个真实窗口，并以基准测试脚本作为其 *shell*，
因此测量覆盖了用户实际体验到的完整输入路径：

- **lyrel**：在工作目录下使用隔离的 `--config-dir`，其中的 `config.json`
  将 `shell` 设置为测试脚本。每次运行都会重新启动 GUI 和守护进程，
  并在运行结束后将其终止（通过限定在该配置目录范围内的 `pkill`）——
  不会触及日常使用的 lyrel 及其守护进程。
- **Alacritty**：使用 `--config-file <empty>`（隔离用户配置），
  通过 `-o window.dimensions.…` 设置网格，通过 `-e` 指定脚本。
- **Ghostty**：使用 `--config-default-files=false`、
  `--window-width/height`（单位为字符单元）和 `-e`。
  `--window-save-state=never` 很重要：否则 macOS 的窗口恢复机制会覆盖请求的尺寸。
- **Kitty**：使用 `--config NONE` 和 `-o initial_window_width/height=<n>c`
  （`c` 后缀表示字符单元）；脚本以尾随参数的形式传入（不使用 `-e`）。
  `-o remember_window_size=no` 很重要：即使使用 `--config NONE`，
  该选项也默认为 yes，而恢复的窗口尺寸会覆盖 `initial_window_*`。

不对 iTerm2/Terminal.app 进行基准测试：它们需要通过 AppleScript 驱动，
且无法干净地隔离配置。

网格公平性：lyrel 没有尺寸参数，因此以其默认窗口作为参考——
先运行 lyrel，驱动脚本随后读取记录下来的 `grid:` 行，
将其他终端设置为相同尺寸（按字符单元而非像素计算；各终端的字体不同）。

<a id="why-doom-fire-is-patched"></a>
## 为什么要给 DOOM-fire 打补丁

`doom-fire-fps.patch`（由 `setup.sh` 应用）使 DOOM-fire 每 30 帧
将累计平均 fps 写入 `$DOOM_FPS_FILE`。上游二进制文件只会将数值*绘制*出来，
而录制输出流再从中解析数值（使用 `script(1)`）是个陷阱：
在 500+ fps 时，录制文件几秒内就会增长到数 GB，
其磁盘写入会使后续运行速度降低至原来的 1/4–1/8。
fps 的定义保持不变——从火焰开始运行起的总帧数 / 经过的秒数，
与屏幕上绘制的数值相同。

<a id="recording-results"></a>
## 记录与解读结果

本工具集不附带历史成绩。每次比较应保存原始结果，并记录 lyrel commit、各终端版本、构建模式、硬件、系统、网格、字体和机器负载。

- 在同一次测试会话中连续测量各终端，保留每次结果与汇总，不只选取最快一次。
- DOOM-fire 的 FPS 同时受生产端调度、PTY 排空和终端消费影响，不应直接等同于绘制性能。
- `cat` 的完成时间反映输出排空，不代表所有文字已显示；分析持续吞吐和渲染延迟时须分别测量。
- lyrel 内存包含 GUI 与守护进程；对比时保持冷启动、空闲时间和配置一致。

## #441 长文档编辑回放（Windows／macOS）

用 `rustc --edition 2024 scripts/bench/document_history_441.rs -o <生成器路径>` 编译夹具生成器，再执行 `<生成器路径> <新的用例目录>`。每轮用不同目录，父目录需已存在。`replace.md` 与 `typing.md` 的原始字节相同，各为 1,048,576 B，含 UTF-8 BOM、CRLF/LF 混合行尾、非 BMP 字符和无末尾换行。用 Windows 的 `fc.exe /b` 或 macOS 的 `cmp` 与 `expected/` 中对应文件逐字节比较。

为当前 Release 候选创建隔离配置和专用 daemon，并关闭 PATH 安装、更新检查和自动下载。通过 `lyrel-app --config-dir <隔离配置目录> --open-path <夹具绝对路径>` 打开首个用例。随后可通过文件侧面板打开同一目录中的其他夹具。保持默认 Markdown 即时编辑、下述键序列、窗口布局、缩放和显示刷新率一致。编辑历史测量时使用英文键盘，系统 IME 验收单独执行。参见[配置隔离说明](../../docs/customization/settings.mdx)。

1. `replace.md` 从第 2 行第 1 列开始，每行选中开头 `a` 并输入一次 `x`。Windows 可先 `Ctrl+G` 输入 `2:1` 确认，随后用 `Shift+Right`、`x`。每次输入显示后，用 `Down`、`Home` 到下一行，再选区、输入。第 2–101 行各改 1 次。只把每次 `x` 到显示正确行的新字形计入输入样本。最后保存并比较 `replace-100.bin`。
2. 用编辑器的普通 Undo 连续撤销 100 次。每步恢复对应的 `a` 单字符选区，光标在选区末尾。第 50 次和第 100 次分别保存并比较 `replace-050.bin` 和 `original.bin`。再 Redo 100 次，每步变回 `x` 并折叠选区至字后。第 50 次和第 100 次分别保存并比较 `replace-050.bin` 和 `replace-100.bin`。耗尽时再执行对应命令没有效果。
3. `typing.md` 同样从第 2 行第 1 列开始，无选区。连续发送 100 次单键 `x`，期间不移动光标、不粘贴、不执行其他编辑。每次输入后光标前进一个字符，按已显示的 `x` 数核对帧与输入的对应关系。保存并比较 `typing-100.bin`。一次 Undo 恢复 `original.bin` 和行首光标。一次 Redo 恢复全部输入，光标在最后一个 `x` 后。两次均保存比较。
4. 在替换完成的 `replace.md` 首行选中 `🎉`，通过实际系统中文 IME 提交“你好”，检查候选、提交后的光标和 `ime-commit.bin`。一次 Undo 恢复原字节和该字符之后的光标。再次选中 `🎉` 并触发预编辑，用 Escape 取消，保存后仍为 `replace-100.bin`。执行 Redo 应能恢复已提交的“你好”。保存、关闭最后一个视图，再重开检查字节和新会话历史。

在保留硬件窗口内交替运行基线和候选。记录提交、构建 profile、硬件、系统、显示、回放命令、固定输入节奏、样本数和轮间波动。每种输入先采空闲，再采已打开文档、编辑期峰值、最后一次输入显示后仍保留历史时的值，随后保存并在关闭最后视图后按同一延迟采样。同一实例用新的夹具副本重复打开、编辑、关闭，记录回落和后续增长。采集 GUI 和 daemon 的进程 CPU、平台内存计数器与适用 GPU 计数器，配合显示事件报告输入 p50/p95 和未匹配样本。分别报告替换与连续输入的保留历史内存增量、事务数和相对打开文档时的增量每事务。验收保存及 IME 过程另记，避开输入采样窗口。

可在预备阶段运行 native 字形消费者，Windows 使用 `pwsh scripts/cargo-windows.ps1 test --release --locked -p lyrel --features native-markdown-tests --test native_markdown -- document_history_441`，macOS 使用同参数的 `cargo test`。它经工作台输入处理器、Undo/Redo、Host 保存和关闭走一遍同一夹具，并断言各事务的选择、光标、精确保存字节及 IME 回调结果。实窗首个正确显示帧、进程计数器、专用 daemon 和系统 IME 以 Release 实窗回放为验收来源。

## CLI capture 与 exec（#436）

在预留机器上的专用测试账户运行。准备对应版本的 Release `lyrel`、`lyrel-server`，以及一份 Release `cli_e2e` 驱动。每轮交替替换待测版本的 CLI 和 server，驱动保持相同。在 Windows 使用 Git Bash 并通过 `scripts/cargo-windows.ps1` 构建和暂存 ConPTY 文件。

```bash
cargo build --locked --release -p lyrel-cli -p lyrel-server
cargo test --locked --release -p lyrel-cli --test cli_e2e --no-run
"$workload_exe" --capture-exec-workload "$revision" "$cli" "$server" "$bash" > "$samples"
```

`$workload_exe` 取 `cargo test --no-run` 报出的 `cli_e2e` 路径。其余路径分别指向待测 CLI、server 和 Bash。Linux 用上述 headless 入口。在 Windows 或 macOS 的专用桌面会话中，另用相同版本的 `lyrel-app`，在命令末尾加 `--gui "$app"`。测量时保持目标窗格可见、窗口大小固定且没有其他输入。可另跑一轮 `--gui "$app" --inspect`，在计时结束后观察 `verified.last_display_line` 指定的最后一行。

每次命令启动独立 daemon 和窗格并输出一轮 JSONL。`workload` 提供版本、进程 ID、终端几何和负载大小。`sample` 按操作记录 CLI 启动到输出及退出的耗时，成功的 `exec` 还记录到 PTY 命令进程开始的耗时。`verified` 必须出现，表示编号字节序列、完整命令输出、退出状态和同一窗格可继续执行命令均通过。比较时保留原始记录、固定预热次数并报告各操作的样本量、中位数、p95 和波动。资源数据按 daemon、CLI、GUI 和生产进程分别归属。