<a id="terminal-benchmark-harness"></a>
# 终端基准测试工具集

用于对比 lyrel 与 Alacritty、Ghostty 和 Kitty 的可复现吞吐量、FPS 和内存基准测试，遵循
[moktavizen/terminal-benchmark](https://github.com/moktavizen/terminal-benchmark)
的方法。
仅支持 macOS（驱动脚本使用 `/Applications` 路径、BSD `ps` 和 `pkill`）。

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