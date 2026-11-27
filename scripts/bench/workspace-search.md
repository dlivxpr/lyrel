# 工作区文件搜索回放（#438）

## 准备隔离实例

`workspace-search.mjs` 需要 Node.js。它独占创建新目录，并在 `project` 内写入恰好 5,000 或 50,000 个普通文件。项目无 Git 仓库和子目录，因为生产遍历会将目录计入 50,000 项上限。两种大小的八个探针同名，其他文件从 `file_00000.txt` 编号到 `file_04991.txt` 或 `file_49991.txt`。每个文件的全部正文是 `LYREL_SEARCH_PATH=<文件名>` 加 LF。

在预留窗口前构建 Release 二进制并生成夹具。把命令里的 `5000` 换成 `50000` 来准备另一档。正式比较复用同一份 `project`，为每次运行在项目外新建配置目录，复制同一份 `config.json`，并用该目录启动专用 daemon。

Windows 在仓库根使用 PowerShell 7.4+。将待测 Release `lyrel-app.exe` 和配套 ConPTY 文件保留在 Cargo 构建目录，避免触发安装版的开始菜单注册逻辑。

```powershell
$searchRoot = Join-Path $env:TEMP ('lyrel-search-438-' + [guid]::NewGuid())
node scripts/bench/workspace-search.mjs prepare $searchRoot 5000
if ($LASTEXITCODE) { throw 'Search fixture preparation failed' }
$searchConfig = Join-Path $searchRoot config
$searchProject = Join-Path $searchRoot project
$searchApp = (Resolve-Path .\target\release\lyrel-app.exe).Path
Start-Process $searchApp -ArgumentList @('--config-dir', ('"{0}"' -f $searchConfig), '--open-path', ('"{0}"' -f $searchProject)) `
  -Environment @{ LYREL_DATA_DIR = $searchConfig } -PassThru
```

macOS 在仓库根运行：

```bash
search_root="$(mktemp -d)/search"
node scripts/bench/workspace-search.mjs prepare "$search_root" 5000 && \
  LYREL_DATA_DIR="$search_root/config" target/release/lyrel-app \
    --config-dir "$search_root/config" --open-path "$search_root/project"
```

只留一个本地终端窗格，确认 shell 的当前目录就是 `project`。在窗格中执行 `node "<仓库绝对路径>/scripts/bench/workspace-search.mjs" hold`。终端应显示 `LYREL_SEARCH_READY` 及项目绝对路径。独立配置用公开的 `agent_commands` 将该脚本识别为 `pi`，以启用生产消息输入框。该前台进程只等待终端输入。Windows 用已启用 Lyrel shell integration 和 PSReadLine 的 PowerShell，macOS 用 zsh 或 Bash。所有操作结束后将焦点还给终端网格，结束前台命令，关闭自有窗口，再用本轮的二进制执行 `--config-dir <本轮配置目录> --stop-daemon`。

## 两个入口的可见断言

- **全局搜索的「文件」页**。macOS 按 `Cmd+O`。Windows 按 `Ctrl+Shift+P`，搜索「转到文件」（英文 `Go to File`）并按 `Enter` 进入文件页。把鼠标移出结果区。用 `Up`、`Down` 移动高亮，`Enter` 应打开文档。悬停文档页签，提示中的绝对路径必须是 `project` 下该文件，正文必须匹配 `LYREL_SEARCH_PATH=<所选文件名>`。关闭文档并回到同一窗格，再进行下一次打开。
- **终端消息输入框的 `@` 菜单**。在夹具窗格按 `Ctrl+Shift+I`（Windows）或 `Cmd+I`（macOS）。确认输入框获得焦点。按表输入查询之前，先输入 `@`，光标始终在词尾。选中项目后按一次 `Enter`，菜单应消失。接着键入 `END`，草稿必须精确显示 `@<文件名> END`。终端若出现 `LYREL_SEARCH_UNEXPECTED_INPUT`，则本轮失败。清理时在输入框中全选并删除草稿。无候选场景只观察结果，`Enter` 会走正常发送路径。

每个入口独立预热。等待空查询浏览行出现（输入框用 `@`），再分别查询当前规模最后一个编号 `file_04991.txt` 或 `file_49991.txt`，以及最后一个按名称排序的 `f文-b.txt`。两次都应只有一行，且默认高亮该行。50,000 档的文件页空查询上限提示应显示 50,000。清空查询再开始采样。热索引采样保持文件页打开，输入框在索引就绪后的 30 秒时效内完成一批。重开文件页或输入框索引过期触发重走时，标记重走期间的样本。

| 查询 | 精确行序和选取 | 最终路径 |
| --- | --- | --- |
| `f` | `fable.txt`、`fritz.txt`、`f文-a.txt`、`f文-b.txt`、`flare-a.txt`、`flare-b.txt`、`frost-a.txt`、`frost-b.txt`。`@` 菜单到此结束。文件页继续按编号显示 `file_00000.txt` 至 `file_00191.txt`，共 200 行。默认高亮 `fable.txt`，按 `Down` 后高亮 `fritz.txt`。 | `fritz.txt` |
| 逐键输入 `file_04242.txt` | 唯一行且默认高亮 `file_04242.txt`。 | `file_04242.txt` |
| 一次 Unicode 文本提交 `文` | `f文-a.txt`、`f文-b.txt`。默认高亮第一行，按 `Down` 后高亮第二行。 | `f文-b.txt` |
| 快速打字与退格 | 先使 `frost` 显示 `frost-a.txt`、`frost-b.txt`，按 `Down` 选第二行。连续发四次 `Backspace`、`l`、`a`、`r`、`e`、`x`、`Backspace`。目标键间距为 8 ms，记录实际间距。最终 `flare` 只显示 `flare-a.txt`、`flare-b.txt`，恢复首行高亮。按 `Down`、`Up`、`Down`，高亮须依次为第二、第一、第二行。 | `flare-b.txt` |

正确性轮逐项按 `Enter`，并滚动核对文件页宽查询的全序。快速回放结束后继续观察至少 1 秒。最终查询显示后不得出现对它无效的可选行，正确行也不得被旧查询覆盖。再重复快速回放，仅省略最后一个退格。最终 `flarex` 应持续显示无结果（文件页）或关闭候选菜单（输入框）。记录选择和路径，保留错误帧。跨越真实索引重走时，观察重走结果落地前后的查询和行序。

## 采样和已有证据

- 复用 #430 的键事件与已显示帧关联，以及 #429 的 OS 计数器。将 `f` 的按下、选择性查询的最后一个键、`文` 的提交和快速回放的最后一个退格，分别关联到首个同时显示目标查询、正确可见行序和正确高亮的帧。通过键盘确认验证结果可选。正确性轮与连续采样使用同样的查询轨迹及前置状态。各入口、索引大小和动作独立统计 p50、p95、超过两个显示刷新周期的比例和未配对数，并记录合帧未独立显示的中间查询及错误帧。
- 同机交错重复 Release 基线和候选，暂停其他工作负载。每个版本至少跑三轮，每轮为每个入口、索引大小和动作采集 20 个样本，保留各轮数据。记录源码和二进制身份、硬件、OS、刷新率、窗口大小、键盘输入方式、预热、运行顺序、样本数及轮间波动。分别记录 GUI、daemon 和夹具子进程的 CPU 与内存，包含空闲、活动峰值和关闭后固定时点，并记录 GUI 的 GPU 计数与可用性。把输入、帧和资源原始记录连同回放命令保存。共享实现变化时复跑前台终端回显。以正确性等价且收益超出重复基线波动为通过条件，或记录维护者对取舍的决定。
- 现有工单报告热的 50,000 路径优化编译微基准中，`f` 的中位数为 35.53 ms，`file_4242` 为 0.84 ms。工单未附原始样本、硬件、精确命令、轮间波动或二进制身份。仓库里的 `a_long_index_is_ranked_quickly_enough_to_type_into` 另用内存合成的 50,000 条路径，对 `file_4242` 调用一次 `rank(..., 20)`，断言首行和 2 秒上限。两个产品入口分别最多构造 200 和 8 行，消息输入框在呈现和处理菜单按键时还可再次构造结果。现有数字用于提出宽查询的测量假设，优化决策以真实窗口证据为准。见 [#429](https://github.com/dlivxpr/lyrel/issues/429) 及 [仓库测试](../../src/ui/search/files.rs#L404)。
