# Tern 的 pane 菜单、拖拽与边缘暂存调研

调研日期：2026-10-23。

## 结论与证据边界

1. **用户点名的 `docs.stencil.so/tern` 是 Stencil 的 Tern；`tern.build` 是另一款同名产品，不能混用菜单、快捷键或更新日志。** 前者自述为 Rust 原生工作区，官方 SDK 仓库为 `stencil-hq/tern-sdk`；后者自述为 Go + AppKit 的 coding-agent IDE，下载和 Homebrew 仓库为 `Shubham-1994/homebrew-tern`。[S1][S2][T1][T3]
2. **Stencil 官方文档直接定义了 PiP 的边缘暂存行为，但使用 `stash`，没有把 `park` 定义成 pane 命令。** 官方原文是：“Hides a card against the specified edge, leaving a tab to bring it back.” 因而可以有依据地设计“贴边隐藏，留一个可恢复的边缘标签”，不能声称官方定义了名为 `Park pane` 的独立功能。[S3]
3. **pane 拖拽不是只有分屏：中心区交换，四角浮为目标 pane 的 PiP，方向区用于位置调整。** 这与 `tern.build` 的“tab 拖到中心就移动到 pane 内”不同，不能照搬。[S4][S5][T1]
4. **官方明确列出 pane/PiP 内置命令，但没有在已查页面给出完整 pane 上下文菜单及这些命令的默认快捷键对照表。** 下文将“命令存在”“菜单入口存在”“截图中某行可见”分开，不把命令面板清单冒充菜单截图。[S6][S7]

本轮只读官方网页、SDK 类型声明与官方仓库说明，并创建本报告；未修改 HTML 原型或生产代码，未运行构建、测试或真实 Tern 应用。结论是资料核实，不是 lyrel 的交互验收。

## 一、先分清两个 Tern

| 项目 | 用户指定的 Stencil Tern | 另一个同名 Tern |
| --- | --- | --- |
| 官方入口 | `https://stencil.so/tern` | `https://tern.build/` |
| 文档 | `https://docs.stencil.so/tern/` | `https://tern.build/docs/desktop` |
| 官方公开仓库 | `stencil-hq/tern-sdk`，协议 SDK 和插件示例 | `Shubham-1994/homebrew-tern`，发行包和 Homebrew tap |
| 直接证据 | Rust native；pane 可 splits、tabs、floats | Go + AppKit；每个 pane 有自己的 tab bar |
| 本报告用途 | pane、PiP、stash 的主要依据 | 仅作对照，不证明 Stencil 的菜单和快捷键 |

以上身份信息分别来自各自官网、文档及官方互链仓库，不是根据名称推断。[S1][S2][T1][T3]

`tern.build/changelog` 中 v0.4.0 的 split panes、v0.5.0 的 floating terminal、v0.7.1 的 context menus 都属于右列产品；其中 floating terminal 也不能视作 Stencil PiP 或 edge parking 的实现证据。[T2]

## 二、官方是否直接描述 park

### 已核实的行为

- `pip stash [#<n>] left|right`：把 PiP 卡片藏到指定的左／右边缘，**留下能把它带回来的 tab**。[S3，Stashing, decks and inspection]
- `pip unstash [#<n>]`：恢复暂存的卡片。[S3]
- `pip stash-all`：将当前 tab 的所有 PiP 在暂存／可见之间切换；对应内置命令 `pip_stash_all` 的标签是 `Hide pictures in picture`。[S3][S6]
- PiP 拖拽释放后的落点可以改变角落、改绑 owner、dock 成分屏，或在边缘 stash。官方没有在该页公开落点距离阈值、速度阈值或优先级算法。[S3，Moving and releasing]
- 状态里有 `stashed`、`tab`、`crowded`、`deck`、`slot`、`rect`，表明边缘标签与角落卡片堆叠是可检查的布局状态，不是简单把 pane 删除。[S3]

### 没有找到的定义与限定

- 对官方完整手册 `print.html` 的读取快照进行单词边界、不区分大小写检索，`park` 和 `parking` 均为 **0 次**；`parked` 为 **2 次**，分别是 agent 元素的状态枚举和 CSS 状态类，**不是 pane 暂存定义**。这只限定于本次读取的公开完整手册，不能证明全部内部源码、旧版本或全部 UI 文案永远不存在该词。[S8][S9]
- 官方 pane/PiP 内置命令表有 `pip`、`pip_dock:*`、`pip_corner:*`、`pip_stash_all` 等，没有在该表看到 `park` 命令。[S6]
- 因而本文将用户所说的 **park / edge parking** 对应到“边缘暂存”这一设计语义，**不把 `park = stash` 写成官方术语同义定义**。[S3][S6]

## 三、pane 菜单、命令与快捷键

### Stencil：能确认到什么层级

官方 `context pane|tab` 脚本接口能打开当前 pane 或 tab 的菜单；`pip menu [#<n>]` 能在 PiP 卡片顶部附近打开其上下文菜单。官方控制接口的 `menus` 能读取已打开菜单的行。**这证明菜单及可检查入口存在，但不等于网页已公布每一项菜单顺序。**[S7][S3][S10]

| 相关命令标签 | 官方内置 ID | 官方文档证明的内容 | 默认快捷键证据 |
| --- | --- | --- | --- |
| `Split right` | `split_right` | 命令存在；在目标右侧新建 pane | 已查 pane/PiP 表未提供，不能填猜测值 |
| `Split down` | `split_down` | 命令存在；在目标下侧新建 pane | 同上 |
| `Picture in picture` | `pip` | 命令存在；脚本 `pip float` 把当前分屏 pane 浮到接管其原空间的 owner 上 | 同上 |
| `Close block` | `close_pane` | 命令存在；官方标为 destructive | 同上 |
| `Zoom block` | `zoom` | 命令存在；脚本 `zoom` 切换当前 pane 的 zoom | 同上 |
| `Even out splits` | `equalize` | 命令存在；均分当前 tab 的分屏 | 同上 |
| `Move pane left/right/up/down` | `move_pane:left/right/up/down` | 四个移动命令 | 同上 |
| `Move pane to tab…` | `move_pane_to_tab` | 跨 tab 移动入口存在 | 同上 |
| `Move pane to session…` | `move_pane_to_session` | 跨 session 移动入口存在 | 同上 |
| `Dock picture in picture left/right/up/down` | `pip_dock:left/right/up/down` | 回到 owner 相应一侧的分屏 | 同上 |
| `Move picture in picture to top left/top right/bottom left/bottom right` | `pip_corner:top_left/top_right/bottom_left/bottom_right` | 四角定位命令 | 同上 |
| `Hide pictures in picture` | `pip_stash_all` | 所有 PiP 隐藏／恢复相关入口 | 同上 |
| `Picture in picture over…` | `pip_over` | 命令存在；此命令表没有展开其选取流程 | 同上 |

表中命令名与 ID 来自官方 Built-in Actions；具体布局行为由 Layout、Terminal 和 PiP 页面补足。**此表不是已核实的完整 pane 上下文菜单。**[S6][S5][S7][S3]

快捷键还受内置默认值、keyboard preset、插件和用户 `settings.json.keybinds` 的分层影响，用户层优先。官方提供 `cx.actions:keys()` 查看有效绑定；应以目标安装版本的菜单／Keyboard 设置或该有效映射为准，不能把文档中示例 `tern.bind("ctrl+a>%", "split_right")` 当成默认快捷键。[S11][S12][S6]

文档页顶部 `←/→`、`S`、`/`、`?`、`Esc` 是 **mdBook 阅读器快捷键**，不是 Tern pane 命令快捷键。[S7]

### `tern.build`：明确公布的菜单与快捷键，但属于另一产品

- 其 **tab 右键菜单**列出：pin、move to a split、split terminal right/down、rename、close、close others、close to the right、copy path、reveal in Finder、markdown preview。文档另明确给出 `Move to Split Right / Down`，并在其他章节补充 tab 的 `Colour`。不要把它改称 Stencil 的 pane 抓手菜单。[T1，Panes and tabs、Terminal]
- `⌘D` 向右分屏，`⇧⌘D` 向下分屏，新 pane 打开 terminal；`⌥⌘` 加方向键切换 pane；`⌘W` 关闭 tab；`⌥⌘A` 是 floating terminal。[T1]
- Stencil 自己的动作表中，代码编辑器的 `⌘D` 被列为 `editor.add_next_occurrence`；这进一步说明不能直接把另一产品的 `⌘D = split right` 移植成 Stencil 官方事实。不同焦点上下文仍可能有不同绑定，本轮没有真实应用有效映射可确认 pane 默认值。[S6]

## 四、拖拽、PiP 与 park 落点应如何表现

### 1. pane 拖拽：先预览，释放才提交

官方 `drag-pane <n> [<m> zone]` **只开始拖拽并设置 hover overlay**；完成需要 `drop … text "tern-pane:<id>" move`，取消可用 `drag-end`。状态 `pane_drag` 报告 dragged pane 与 `over` target/zone。可据此区分“当前候选落点”和“已经变更布局”，不应 hover 一下就永久移动。[S4]

| 候选区 | 官方能够直接确认的结果 |
| --- | --- |
| `left/right/up/down` | 官方 pane drag 接受这些方向区；独立 `move-pane` 和布局 `move(pane, target, dir)` 定义方向移动／放到 target 对应侧。[S4][S5] |
| `center` | **交换两个 pane**，不是合并成同 pane 的 tab。[S4] |
| `tl/tr/bl/br` | 把拖入的 pane 浮为目标 pane 左上／右上／左下／右下角的 PiP。[S4] |
| 取消或没有提交 drop | 取消拖拽；官方提供明确 cancel 入口。[S4] |

官方没有在该指南规定 overlay 的颜色、大小、抓手点击／长按阈值；这些属于 lyrel 自己的表现层选择。[S4]

### 2. PiP：有 owner 的浮层，不是无归属的小窗口

- PiP 在当前 tab 中浮于一个 **owner block** 之上；普通 pane 转 PiP 时，接管原空间的 block 成为 owner。没有其他合适 owner 时，`pip float` 失败；不应凭空把最后一个 pane 变成没有归属的卡片。[S3]
- 四角为 `tl/tr/bl/br`。`float` 默认取最接近旧 pane 中心的 owner 角落；新建 browser PiP 默认右下角。[S3]
- `focus` 让卡片变大、变平以便交互；`expand` 在常规 focus 大小与铺满其 frame 之间切换。`glance` 和 focus 大小分别保存；实际尺寸受 frame 约束。[S3]
- `dock` 把卡片还原成 owner 旁的分屏；不传方向时，有原位置则恢复原位置，否则取面向其当前角落的一侧。dock 后不再属于 `pips`。[S3]
- 释放可有惯性，`pip release 0 0` 可无惯性落下；官方示例明确展示跨 pane 拖动后改绑 owner。[S3]

### 3. 边缘 park：可逆暂存，而非关闭或停止

- 官方直接支持的是 PiP **左／右边缘** stash，留下恢复 tab，随后 unstash；不能由此声称官方支持上／下边缘 parking。[S3]
- “拖到边缘后 stash”有官方文字依据；“距离边缘多少像素”“向外甩多快”“是否 hover 即展开”“多个标签如何排序”在已查页面没有明确规则。[S3]
- PiP 还支持角落 deck、fan 与 glance 模式下按 Option 的半透明／点击穿透效果；这些是官方的额外交互，不是实现最小 park 必须照搬的部分。[S3]
- 暂存不同于关闭：`stash` 被定义为隐藏与可恢复；底层 pane 在 daemon 内运行、窗口只是镜像是官方架构。但本轮没有实测暂存期间的进程状态或重启恢复，不对此作运行验证承诺。[S3][S13]

## 五、截图观察与官方事实不能混写

本次子任务上下文**没有提供可直接查看的用户截图文件**，所以这里不伪造截图逐行转录，也不宣称看到某个快捷键、箭头或 Park 标签。以下是整合用户截图时的证据归类规则：

| 若截图中出现这些内容 | 可以记录的观察 | 不能仅凭该图推出的结论 |
| --- | --- | --- |
| pane 抓手旁展开的菜单 | 当前图中可见的条目、顺序、分隔线、键帽文字 | 所有 pane 类型、所有版本、所有系统都相同 |
| `Split right`、`Split down`、`Picture in picture` | 截图当时显示了这些菜单行；这些命令另有官方文字证据 [S6] | 同菜单的全部条目已被 SDK 文档逐一定义 |
| `Park` 或 edge 标签 | 当前版本／状态的界面文案或形态 | `park` 是公开 SDK 命令，或与 `stash` 完全等价 |
| 拖拽时的高亮边／角／中心 | 可见的候选落点提示 | 释放后的结果、惯性阈值或取消行为；这些需官方文字或实际连续操作 [S4][S3] |
| 贴边小卡片／窄标签 | 当前可见的边缘形态 | 进程是否继续、点击还是 hover 恢复、是否跨 tab／重启保持 |

Stencil 官网还有一个明确标注“a picture in picture over omp”的演示卡片，能作为**官方展示的视觉参考**；它不是本次实际操作中的完整菜单／drag／stash 行为验证。[S1]

## 六、对 lyrel HTML 原型的最小建议

以下均为 **[INFERENCE] 设计建议**，不是声称已经在 lyrel 实现或逐像素复刻 Tern。

1. **抓手保留单一入口，菜单先精简为“向右拆分／向下拆分／画中画／关闭”。** 前三项有 Stencil 内置命令依据；关闭与暂存分开，避免用户把隐藏误认为停止。没有目标平台有效绑定证据前，不给菜单硬填 Tern 默认快捷键。[S6][S11]
2. **拖拽 overlay 标明行为而不只画一片色块**：边区显示“移到右侧／下方”等，中心显示“交换”，四角显示“画中画”。只有释放到有效区域才提交；取消回到原布局。中心“交换”尤其不要照搬另一个 Tern 的 tab 合并语义。[S4][T1]
3. **将 park 作为 PiP 的边缘暂存状态。** 最小闭环是：pane → PiP → 左／右边缘标签 → 点击恢复 PiP → dock 回分屏。标签显示来源 pane 名称，保留明确恢复入口；不把 park 做成仅弹 toast 或直接移除 DOM。来源 pane 名称和点击触发方式是 lyrel 的设计选择，官方只保证“留 tab 可恢复”。[S3]
4. **PiP 至少记录 `paneId`、`ownerId`、corner、原分屏位置、stash side。** 原位置供 dock 恢复，stash side 供边缘呈现；选中或拖动只改变呈现／位置，不生成新会话。若仅余一个 pane，没有合适 owner，则禁用 PiP 并解释原因。[S3]
5. **原型无需模拟甩动惯性、deck fan、Option 穿透。** 保留四角吸附、边缘暂存预览、恢复与 dock 即可表达用户要求的最小闭环；不要为了表面相似引入未经证据支持的复杂阈值。[S3]

## 一手来源索引

- **[S1] Stencil Tern 官网**：https://stencil.so/tern — 产品身份、Rust 原生、blocks 的 splits/tabs/floats、PiP 展示。
- **[S2] 官方 SDK 仓库**：https://github.com/stencil-hq/tern-sdk — 官网／文档互链，协议 SDK 与插件示例；不是整款 Tern UI 内部源码。类型声明：https://github.com/stencil-hq/tern-sdk/blob/main/plugins/tern.d.luau 。
- **[S3] Pictures in Picture**：https://docs.stencil.so/tern/scripts/pip.html — owner、角落、focus/expand/dock、drag/release、stash/unstash、deck 与状态；本报告最关键行为来源。
- **[S4] Tabs and Navigation / Moving panes**：https://docs.stencil.so/tern/scripts/navigation.html#moving-panes — pane drag 的方向区、中心交换、四角 PiP、hover overlay 和提交／取消。
- **[S5] Layout and Workspaces / Arranging panes**：https://docs.stencil.so/tern/guides/layout.html#arranging-panes — split/move/resize 方向、焦点及跨 host 限制。
- **[S6] Built-in Actions**：https://docs.stencil.so/tern/reference/actions.html#blocks-and-panes 、https://docs.stencil.so/tern/reference/actions.html#picture-in-picture — pane/PiP 命令名与 ID；完整页也列出 editor 的默认快捷键。
- **[S7] Terminal and Panes / Panes, tabs and menus**：https://docs.stencil.so/tern/scripts/terminal.html#panes-tabs-and-menus — split、zoom、equalize、`context pane|tab`。
- **[S8] 官方完整手册**：https://docs.stencil.so/tern/print.html — 本次 `park`／`parking`／`parked` 检索范围；不是搜索引擎摘要。
- **[S9] Tool, Agent and Task Kinds**：https://docs.stencil.so/tern/elements/work.html — 完整手册中 agent 的 `parked` 状态对应章节；`parked` 是 agent 呈现状态，不是 pane parking 说明。
- **[S10] Control Commands**：https://docs.stencil.so/tern/scripts/control.html — `menus`、状态及菜单检查能力。
- **[S11] Commands, Keys, and Overrides**：https://docs.stencil.so/tern/guides/commands.html#keymap-layering — 默认／preset／插件／用户层及覆盖优先级。
- **[S12] Window API / Actions cx、Settings cx**：https://docs.stencil.so/tern/reference/api-window.html#actions-cx 、https://docs.stencil.so/tern/reference/api-window.html#settings-cx — `cx.actions:keys()` 是有效映射，`cx.settings:keybinds()` 仅是用户覆盖。
- **[S13] Architecture**：https://docs.stencil.so/tern/concepts/architecture.html#why-the-split-exists — pane 与权威屏幕在 daemon，窗口退出 pane 继续。
- **[T1] 同名产品 Desktop 文档**：https://tern.build/docs/desktop#panes-and-tabs — tab 右键菜单、⌘D／⇧⌘D、tab drag；https://tern.build/docs/desktop#terminal — floating terminal。
- **[T2] 同名产品 Changelog**：https://tern.build/changelog — v0.4.0、v0.5.0、v0.7.1 等记录，仅属于该产品。
- **[T3] 同名产品发行仓库**：https://github.com/Shubham-1994/homebrew-tern — 发行包／Homebrew tap、自述 lightweight IDE。
