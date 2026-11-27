# 结构树与表格性能工作负载（#443）

使用 Windows 或 macOS 上的隔离 Release 工作台。`443-tree.jsonc` 包含 3,500 个嵌套对象、两个同名 `phase` 属性、BOM、混合行尾、注释、转义和精确数字字面量。`443-table.csv` 以仓库现有的 `table.csv` 为种子，加上固定探针和 50,000 条记录，共 50,005 条逻辑记录。表格保留 BOM、引号内逗号、多行字段、转义引号、混合行尾、短行和空字段。两个文件都低于各自的投影预算。

## 准备

1. 在测量窗口前构建基线与候选的 Release GUI 和 `lyrel` CLI。Windows 使用 `pwsh scripts/cargo-windows.ps1 build --release --locked -p lyrel -p lyrel-cli`，macOS 使用 `cargo build --release --locked -p lyrel -p lyrel-cli`。
2. 准备只用于本轮的目录和文件。用新路径重做每个完整编辑样本。保存命令的 JSON 输出以记录文件大小和 SHA-256，基线和候选使用同一份工作负载脚本。

   ```sh
   node scripts/bench/structured-documents.mjs prepare <new-root>
   node scripts/bench/structured-documents.mjs verify <new-root> original all
   ```

3. 从待测 Release 产物启动 `lyrel-app --config-dir <new-root>/config`。在单独的测试 shell 中把 CLI 的 `LYREL_CONFIG_DIR` 指向同一个配置目录，运行 `lyrel new <new-root>/workspace --open`，用 `lyrel status --json` 记录专用 Session Daemon 的 PID 和 socket。选择新工作区，在右侧「文件」工具等待两个测试文件可点击。此次启动与工作区准备在文档输入计时之前完成。
4. 固定窗口、缩放、主题、字体、语言、刷新率和停靠布局，保持终端及文档同时可见。每批使用相同的预热次数和文件顺序。测量窗口中暂停无关构建、测试和 GPU 工作；在同一机器上交替运行基线和候选。关闭本轮工作台窗口后，只用本轮配置目录调用 `lyrel-app --config-dir <new-root>/config --stop-daemon` 来结束专用 daemon。

## 重放

由原生输入驱动实际产品窗口。以下 `Ctrl` 在 macOS 上用 `Cmd` 替代。在每次切换或编辑前等待上一项显示预期内容。记录重放的输入方式和时间戳。

| 操作 | JSONC | CSV |
| --- | --- | --- |
| 打开 | 点击文件树中的 `443-tree.jsonc`。首个可用帧显示该文档的结构树和 `TREE-READY`。 | 点击 `443-table.csv`。首个可用帧显示逻辑记录 `table-ready` 和单元格 `CELL-1111`。 |
| 展开与移动 | 展开 `payload`，检查索引 `0` 出现，再折叠。展开 `probe` 下的 `target`，选择第二个 `phase`，其值为 `"TREE-1111"`，第一个值仍显示 `"first\u4e2d"`。 | 选择 `table-ready` 记录的第三个字段，确认选中 `CELL-1111`。 |
| 定位源码 | 按 Enter，检查选择精确落到第二个 `"phase": "TREE-1111"`，通过工具栏回到结构视图。 | 按 Enter，检查选中源码里的 `CELL-1111`，通过工具栏回到表格视图。 |
| 格式编辑 | 选中同一节点，点击 `Edit value`，在字段中输入含引号的 `"TREE-2222"`，点击 `Apply`。等待树显示新值。 | 选中同一单元格，按 F2，输入 `CELL-2222`，按 Enter 提交。等待表格中的该单元格显示新值。 |
| 切换修订 | 在结构树取得焦点时按 Ctrl+Z，等待 `TREE-1111`，再按 Ctrl+Shift+Z，等待 `TREE-2222`。 | 在表格取得焦点时按 Ctrl+Z，等待 `CELL-1111`，再按 Ctrl+Shift+Z，等待 `CELL-2222`。 |
| 连续更新 | 选中第二个 `phase` 并按 Enter 定位源码，在最后一个 `2` 和右引号之间放置光标。连续发送 Backspace、`9`，立即通过工具栏返回结构视图。 | 选择探针单元格并按 Enter 定位源码，光标放在 `CELL-2222` 末尾。连续发送 Backspace、`9`，立即通过工具栏返回表格视图。 |
| 确认当前修订 | 返回结构视图后，首个可用投影应显示 `TREE-2229`，并持续显示该值。 | 返回表格后，首个可用投影应显示 `CELL-2229`，并持续显示该值。 |
| 保存与重开 | 保存，核对 `edited tree`，关闭文档，再从「文件」独立重开。展开 `target` 核对 `TREE-2229` 和未改的第一个属性。 | 保存，核对 `edited table`，关闭文档，再独立重开，核对探针单元格 `CELL-2229` 和原始多行字段。 |

开始前检查 `verify <root> original all`。每个格式完成编辑与修订切换后、保存之前，用 `verify <root> original tree` 或 `verify <root> original table` 检查该文件仍为原始字节。按 Ctrl+S 等待文档的未保存标记消失，再检查落盘字节。完成两个格式后，关闭所有文档并复核两份文件。

```sh
node scripts/bench/structured-documents.mjs verify <root> edited tree
node scripts/bench/structured-documents.mjs verify <root> edited table
node scripts/bench/structured-documents.mjs verify <root> edited all
```

连续更新阶段保留返回投影后的画面，检查 `TREE-222`、`TREE-2222` 或 `CELL-222`、`CELL-2222` 没有作为当前修订重新出现。遇到加载状态继续记录，直到看到正确的投影。另在采样窗口之外复用确定性暂停解析用例，验证迟到任务不能安装旧修订或写回源文。

```sh
cargo test --locked -p lyrel --lib invalid_budget_and_late_results_preserve_source_and_never_publish_stale_structure
cargo test --locked -p lyrel --lib utf16_tsv_source_history_and_stale_parse_keep_exact_values
cargo test --locked -p lyrel --lib a_stale_table_cannot_write_back_while_the_source_is_reparsing
```

Windows 将 `cargo test` 替换为 `pwsh scripts/cargo-windows.ps1 test`。上述断言使用生产工作台消费者与既有解析门闩，原生连续更新使用未加门闩的产品构建。

## 采样记录

- 按 #429 的显示测量方法记录输入到首个显示正确结果的帧。给 `open`、`expand`、`source`、`edit`、`undo`、`redo`、`current` 分别保留原始时间戳和帧证据。`current` 从最后一次字符输入开始，另外记录紧随其后的视图切换时刻，便于区分操作间隔与投影等待。每项报告样本数、p50/p95、超过两个刷新周期的比例、未匹配数和各轮波动。原生显示事件与画面中的已知文档名、模式和探针一起确认结果；输入框里尚未提交的值不能作为投影结果。
- 加载前、展开或编辑期间、保留打开文档、关闭最后一个文档后采集 GUI 和 daemon 的进程内存。分别保存区间峰值与关闭后 2 秒和 10 秒的值，注明平台和内存单位。单独重复打开已保存的两个文档、等待投影、关闭所有文档，比较同一进程中多轮释放后的读数。记录 GUI/daemon CPU 时间与 GUI GPU 计数器、采样间隔和实际硬件；缺失的计数器标为未采集。
- 在相同的停靠布局下，沿用终端回显工作负载采集空闲对照和解析期间的回显。该有回显运行与无回显运行分别留档。记录提交、构建模式、OS、CPU、GPU、驱动、刷新率和窗口参数、重放命令、语料 SHA-256、进程标识、预热、采样窗口及变异。保留原始帧记录、计数器和每轮字节检查结果。
- 保留每个格式与平台的独立结论。观察到可复现开销时，对比等价输入与结果、基线波动及终端输入，记录改善或维护者取舍。若保留现状，附基线数据。
