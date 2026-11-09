# lyrel 界面设计规范

本规范约束应用界面，不重写 shell、TUI、代码语法的颜色。实现入口是
`src/ui/presets.rs` 的 `Neutrals`、`Interactions`、`Semantics`，以及
`src/ui/theme.rs` 的组件映射。组件应引用这些角色，不在页面里另配颜色。

## 信息优先级

1. **工作内容**：终端、正在编辑的文档、正在查看的 diff。主区域最大，正文对比度最高。
2. **当前位置与下一步**：当前工作区/标签页、键盘当前选项、可执行的主操作。
3. **辅助导航**：右侧信息/Git/文件页签、展开收起、新建、更多。中性图标，始终可见。
4. **说明与元数据**：路径、分支、数量、快捷键、设置描述。次级文字，不用饱和色抢正文。
5. **异常**：失败、冲突、需要处理的警告。局部状态色，加文字或形状，不单靠颜色。

同屏不能把所有选中项都画成主按钮。当前标签页用中性灰底 + medium；右侧面板当前页签用
正文色文字 + medium 字重；真正可执行的主按钮才使用实色强调色。每个任务区只突出一个主操作。

## 颜色角色

| 角色 | 使用位置 | 规则 |
| --- | --- | --- |
| `Neutrals.background` | 工作内容、设置内容、输入框 | 安静的基础表面。输入框未聚焦时不能像已选中列表行 |
| `Neutrals.rail` | 左侧标签栏 | 内容底向正文色偏 3%（默认 `#F5F5F3` / `#1E1E20`），细分隔线划界 |
| `Neutrals.sidebar` | 右侧面板 | 与内容同色；只用细分隔线划界 |
| `Neutrals.popover` | 菜单、随处搜索、对话框 | 浅色下保持明亮，深色下抬高亮度；只给浮层阴影 |
| `foreground` | 正文、标题、当前项 | 正常文字至少 4.5:1；不把普通正文全部加粗 |
| `muted_foreground` | 描述、路径、快捷键 | 与主文字明确区分，但仍保持可读性 |
| `border` / `divider` | 控件边界 / 区域分隔 | 控件边界较强，区域分隔较弱；不能把页面画成表格线框 |
| `Interactions.input_border` | 输入框与普通按钮轮廓 | 对应组件库的 `input` 边框角色，不能误用内容背景色导致边界消失 |
| `Interactions.primary` | 保存、连接等主操作 | 强调色实底，成对配置前景色；hover/pressed 保持标签对比度。Git 提交按钮例外：可提交时用反色中性（正文色底），见 Git 面板 |
| `Surfaces.rail.selected` | 当前标签页 | 中性 selected 阶 + medium；不用强调色，蓝色留给可按的东西 |
| `Interactions.choice` | 菜单的键盘当前行 | 强调色浅底，区别于鼠标经过的中性 hover。随处搜索例外：与工作区切换器同用中性 selected 阶 |
| 中性 `secondary` | 面板页签、分段选项、普通按钮 | 次级导航不能与主要操作争夺注意力 |
| `Semantics.danger` | 失败、Git 冲突、破坏性动作 | 红；必须同时有文字/图标/确认语义 |
| `Semantics.warning` | 需要留意的状态 | 琥珀；Git 普通修改只用于尾部标记，不能染黄整个文件树 |
| `Semantics.success` | 成功、Git 新增标记 | 绿；不用于装饰性按钮 |
| `Semantics.info/link` | 信息、可点击链接 | 使用当前主题的强调色，与焦点指示同族 |

默认浅色以暖白 `#FCFCFB` 为底，正文 `#1C1C1E`，蓝色 `#1F6BF0` 为强调色（光标同强调色）。
默认深色以 `#18181A` 为内容底，`#ECECED` 为正文，`#78A8F5` 为强调色。
Git 的新增/修改标记种子为绿 `#2F8A52` / `#4CC27A`、琥珀 `#B7791F` / `#E0A345`（浅/深），再按对比度下限校正。
其他主题保留自己的背景与强调色，但红/绿/琥珀的**状态含义**不随 ANSI 槽位变化。
所有文字、状态色和主按钮前景色须在实际背景上验证对比度。

## 窗口材质与表面

- 原生材质属于整个窗口；终端、左侧标签栏和右侧面板共用现有窗口背景策略，不创建侧栏专属 blur 或子窗口。
- 表面是否透明只取决于已解析的窗口不透明度，不取决于 Windows 下拉框是否显式选择材质。`Auto`、`Off` 的手动透明度以及 macOS 的本地模糊开关均使用同一规则。
- 不透明度小于 `1` 时，两侧栏使用各自语义底色作 tint，而非另一层原生模糊。Windows 保留已有 `0.15 × (1 - opacity)` 绘制 alpha，其他平台保留 `0.15` src-over 路径。本次隔离 Windows 原生黑／白背景对照中，原始 Pre `0.3.10`、Kit/Pre `0.3.11` 与 Fast `0.1.13` 在浅色、opacity `0.88`、backdrop `off` 的三处空白采样 RGB 完全一致：终端黑白差为 `31`，两侧栏为 `30`。这证明该场景保留了既有外观，不证明旧文档所称的 `85%` 透出目标；实测比例约为 `96.8%`，偏差在升级前已存在，本次不顺带改变。三者普通 DirectX alpha blend 均为 `ONE / INV_SRC_ALPHA`，不能再用更旧 `0.3.9` 的累加 alpha 解释当前行为。不透明度为 `1` 或未启用透明度时恢复完整不透明语义底色；背景图片与渐变继续由窗口层绘制。其他主题、材质与平台仍按以下规约分别验收。
- `Neutrals.rail`、`Neutrals.sidebar` 及文字、分隔线、hover、selected、focus 角色仍由当前主题解析。控件、当前行和遮盖行尾文字的局部操作底可保持实色，以免标签与下方文字混叠；不得因此给整个侧栏重新加不透明底。
- `theme.sidebar` 本身仍是不透明 token；设置、文件与 diff 等全窗口覆盖层维持其既有不透明策略，不能把工作区 tint 直接写回全局 token。
- 材质切换不重置主题、界面或终端字体，也不重置用户的 opacity。优先级仍为配置 override、主题值、平台材质默认值；Windows `Auto` 不因更新自动变透明。
- Windows 继续使用现有 DWM/WCA 配置、版本回退及激活／DPI 重放；macOS 保留本地 blur 和原生 appearance。Mica 不是实时桌面模糊，不承诺浏览器 blur radius 或 saturation 的逐像素等价。
- 颜色测试只证明已知主题底色上的文字与控件对比度，不能证明任意桌面、背景图片或原生合成后的实际对比度。集成后必须使用隔离配置，分别检查浅色、深色、代表性自定义主题，往返切换 opaque／透明／各平台材质，检查两侧栏收起、展开、拖拽 resize、窗口激活与失活及焦点可见性；逐平台记录实际结果，不用 Windows 证据代替 macOS。
- alpha 字段或绘制场景中的颜色值正确，不代表 Windows 最终合成结果正确。透明度验收必须使用窗口后的受控黑／白背景做像素差分；blur 验收必须对同一高反差图案切换开／关作比较，并排除其他窗口遮挡、最小化及采样落入文字的情况。

## 状态不能混用

- Rest：中性、稳定。工具按钮不因鼠标移出而消失。
- Hover：轻微中性填充，只用于可交互目标。设置说明行不整体发亮。
- Selected：表达持久的当前位置。侧栏当前行用中性底 + medium，右侧页签用正文色 + medium 字重，不画横条也不画底。
- Keyboard focus：清晰的强调色焦点环/当前行，不能仅靠 hover 表达。
- Pressed：同一色系的短暂加深，不能跳到另一个主题色。
- Disabled：保留边界与标签；降低对比，不能像可执行的主按钮。
- Error：状态色、说明和恢复入口一起出现。普通 Git 修改不升级为错误/警报。

标签页关闭按钮、渐变遮挡等局部覆盖层必须使用所在行的**实际**背景，不能复用过期色值。

## 图标、文字与间距

- 界面 SVG 使用 24×24 坐标系、1.8 线宽、圆端点；工具栏按 16px 显示，命中区 32px。
- 左右侧栏切换按钮在展开和折叠状态保持相同尺寸与垂直中心；右侧切换按钮距窗口右边缘的中心距离也保持不变。
- 侧栏切换图标以对应侧栏区域的填充表示可见状态：展开时填充，收起时不填充；外框和分隔线保持不变。
- 行内辅助按钮至少保留现有 24px 命中区域；增大图标不能挤掉文本预算。
- 工具图标使用中性文字色。品牌用小图形识别，饱和色留给状态点。
- 字号跟随界面缩放；正文 14/16rem，说明 12/16rem，章节标题 16/16rem。
- 左侧栏分组标题为 11.5/16rem、medium、辅助色；右侧紧凑分组标题为 12/16rem、semibold；设置页只有页标题为 16/16rem semibold，页内分组标题与信息页同为 11.5/16rem medium 辅助色。Session 字段名和值同为正文尺寸，以颜色区分层级。
- 辅助文字必须在实际的内容、侧栏、浮层背景上均达到 4.5:1；普通路径、数量和说明不再叠加透明度。静态 PID 使用中性文字。
- 放大界面字号时，Git 行高与设置导航宽度同步调整；窄侧栏优先保留分支名，长文件名须明确省略并可查看完整路径。
- 侧栏当前标签页标题用 medium（测宽与渲染同一字重），分组标题用 medium；正文和辅助说明保持 regular。
- 右侧面板和侧栏的紧凑标题不再大写，靠字重与颜色区分层级。
- 以 4px 为间距基础：紧密内容 4–8，行内 8–12，区块 24–32。列表外边距与文字预算同步。
- 形状：侧栏标签行 `ROW_RADIUS`（7px），工作区切换器圆角；窗口左右各一个圆角三按钮组，按钮命中区 `2rem`、图标 `1rem`，展开和折叠时尺寸与垂直中心相同。浮层 `POPOVER_RADIUS` 10px。
- 窗口 chrome 始终为完整的透明顶栏，高 `3rem`，左侧为品牌、侧栏开关／随处搜索／新标签页，右侧为向右分屏／向下分屏／面板开关，中间留空作为窗口拖动区，不增加实色背景、阴影或贯穿底线。左右按钮组锚定窗口两端，不跟随侧栏宽度移动。两侧栏展开、仅左栏收起、仅右栏收起和两栏都收起时，顶栏高度、按钮位置与终端顶部保持一致；折叠只改变内容区可用宽度，不改变终端行数预算。界面缩放时统一调整顶栏和产品控件尺寸，macOS 交通灯与 Windows 原生按钮仍保留原生尺寸和操作。按钮之外的顶栏空白可拖动窗口或执行平台标题栏双击行为；终端从顶栏下沿开始，首行不被按钮或拖动层覆盖。
- macOS 仅窗口模式保留原生交通灯占位；进入全屏后，品牌与右侧按钮组分别距窗口两端 `0.75rem`，随界面缩放。全屏右侧使用既有同高拖动行，不引入 `TitleBar` 的额外全屏占位；退出全屏恢复原来的交通灯与两端布局。Windows 的全屏与非全屏布局不随此规则变更。
- 侧栏单行标签页行高 30px，带分支行 42px，行间距 1px，组间距 16px；头像 16px 圆形品牌底；分支行 11.5px 辅助色、从开头省略；运行中的 agent 在行尾显示 5px 状态点。工作区切换器独占顶栏下的下一行，显示真实首字母头像、名称和切换箭头，不再重复提供标签过滤框。

## 各界面约束

- **侧栏**：名称优先于分支和 diff 数量；当前项清晰，品牌头像不形成彩色墙。分组标题用辅助色
  medium（22px 行），右侧 `分支 · +增 −删` 为等宽数字的辅助色；展开的分组不画 chevron，折叠后才显示。
- **侧栏顶部**：两侧栏均从固定顶栏下沿开始，导航与内容用留白衔接，不加贯穿的 header 底部分隔线；左栏是唯一带底色的侧面，右侧面板与内容同色。左右竖向分隔线只属于内容区，使用 `hairline(window)` 和 `sidebar_border`，不穿过顶栏，也不为顶部按钮另留一行。
- **右侧面板页签**：文字页签（信息 / 更改 / 文件），12.5/16rem，首个页签距左缘 22px、
  页签间距 18px；当前页签正文色 + medium，其余辅助色，hover 只把文字提到正文色。页签栏
  下方不画横条、不画分隔线。「更改」后附改动数（辅助色、regular）。窗口三按钮组与该行分开，折叠面板不影响窗口控件。
- **信息页**：Session / Processes / Ports 三节，节间 16px、不画分隔线；节标题 28px 高、
  11.5/16rem medium 辅助色。Session 为两列（标签列至少 76px，行高 28px），值用正文色；
  cwd 的父路径弱化、末级正文色，从头部省略。进程行 26px，子进程以 `└` 缩进，PID 辅助色
  mono。Ports 标题尾部带「+」转发按钮，空状态一行辅助色说明。
- **右侧文字层级**：文件树名称使用侧栏标题色，不能整体降为辅助灰；当前项使用 selected 文字色与 medium 字重，根目录以字重区分。Git 普通文件名使用 resting 文字色与界面字体，路径、数量与说明保持次级；信息页的路径前缀弱化，末级名称突出。
- **文件树**：顶部是 28px 的浅底搜索框（圆角 7）。行高 26px、圆角 6；目录前有展开 chevron，
  文件留空同宽；文件名中性；A/M/? 等尾部字符承载 Git 状态；目录用 5px 小点表示子项变化；
  忽略项文字辅助色、图标半透明，不用斜体；冲突才整行提升。
- **Git 面板**：改动列表是主体。顶部固定块（分支行 28px / 提交信息框 / 提交行）上 8、块间 10、下 14。
  分支名 medium，末尾同步按钮 26px。提交信息框静止 56px、淡底、`ROW_RADIUS`。
  提交按钮是分体控件（26px、圆角 6、0.5px 内嵌分隔）：可提交时整体反色中性（正文色底 + 表面色字），
  这是面板里唯一的实心形状；不可提交时回到信息框同款淡底，不宣传不能执行的动作。
  分组之间空 16px，组标题 22px、11.5/16rem medium 辅助色、句首大写（不再全大写），数量右对齐。
  文件行 26px、圆角 6：状态字母等宽 semibold 按状态着色，文件名正文色优先占宽（下限 40px），
  目录右对齐、从开头省略。History 区以细线分隔，标题 32px；提交行 26px，1px 连线 + 7px 节点，
  HEAD 实心、其余空心环；整页只有一条 lane 时连线与节点改用中性色；时间列固定宽度右对齐，
  HEAD 引用用淡底胶囊。
- **设置**：页标题 > 分组标题 > 标签 > 描述。左侧导航用 `Neutrals.rail` 底与其 surface 阶，右缘细分隔线；
  搜索框 28px 浅底（圆角 7）；导航行 28px、圆角 7、左右内边距 8，当前项为 selected 阶 + medium，不用强调色。
  页标题立在 48px 标题栏带内；分组标题 28px、11.5/16rem medium 辅助色；分组之间是 0.5px 分隔线，上下各 16px。
  设置行：标签正文色 regular，描述 12/16rem 辅助色，行最低 28px、内边距 8（hover/搜索命中的底色外扩 8px）；
  搜索命中行用浅中性底，不用强调色。输入框与下拉框 28px，按钮、步进器 26px（圆角 6）；
  输入框/下拉框/次要按钮都用同一浅底（surface 半阶）且无描边。控件一义一种：开关管开/关，下拉管单选
  （不再用分段控件），滑块管连续值，步进器管整数微调。
  每个视图唯一的主操作（保存主题草稿、连接、安装更新）用反色中性，同 Git 提交按钮；不可执行时回到浅底。
  开关与滑块仍用强调色。SSH 主机列表：标题行带 26px 图标按钮，28px 浅底搜索，分组标题 22px，
  主机行 42px 两行（标题正文、地址 11.5/16rem 辅助色）；主机表单字段名为辅助色右对齐标签列。
  主题卡片为浅底无描边，展开时为 selected 阶；主题面板与内容同色、左缘细分隔线，标题立在标题栏带内。
  快捷键键帽为浅底无描边，行间 0.5px 分隔线。
- **菜单与随处搜索**：亮度抬升、有限阴影；当前键盘行清楚，普通项不着色；小窗口内可滚动。
- **框架菜单例外**：`gpui-kit 0.9.0` 的 `PopupMenu` 沿用正式框架默认几何，包括 2px full-bleed 分隔线、26px 行和默认 rem=16 时的 14px 字。应用自有设置菜单保持既有约束；不为恢复旧菜单几何复制框架私有实现。Fast 下深色 Blurred 的像素与列表提示须按本次场景重测，不沿用旧 backend 的原生结论，也不扩大为任意视觉或交互降级。
- **工作区切换器**：760px 浮层，距顶 112px，圆角 12px；搜索行 48px（右侧 `esc` 键帽），
  主体 420px 分两栏、只用细线分隔，页脚 40px（左「新建工作区」幽灵按钮，右键帽提示
  ↑↓ 导航 / ↵ 打开 / ⌘↵ 新窗口）。左栏 340px 工作区行 52px：26px 首字母圆 + 8px 连接
  状态点（在线绿、离线淡、连接中琥珀、失败红，外圈为行底色），名称 medium，下行
  「机器 · 路径 · 时间」次级文字，右侧标签数 + 状态词。右栏为预览：28px 标题行，
  44px 标签页行（18px 品牌头像、分支与增删数全部次级、「当前」字样、工作中 5px 绿点
  与侧栏同步闪烁）。当前行只用中性 selected 阶，不用强调色。
- **随处搜索**：切换器的同胞浮层——同样的 12px 圆角、浮层阴影、距顶 112px（矮窗口按比例上移），
  最宽 600px；搜索行与页脚共用 40px 高度，列表高度预算也使用同一高度。搜索输入与列表通过公开 API 组合，
  保留筛选、清空、焦点与列表键盘导航；空时右侧 `esc` 键帽由输入框布局垂直居中，不按假定字段高度计算偏移。
  页脚以细线分隔，右侧键帽提示 ↑↓ 导航 / ↵ 打开。命令行 32px、圆角 8、列表四周 8px、行内 10px；当前行为中性 selected 阶 + 标题
  medium，不用强调色。分组标题 28px、11.5/16rem medium 辅助色、与行文字同列。快捷键逐键 18px 淡底键帽。
- **分屏与文档栏**：分屏分隔线静止时为一个设备像素的 divider 色细线，hover/拖动时变 1px 强调色；两侧栏边缘保留真正 resize 与拖动反馈，仅 hover 不显蓝线。文档栏位于窗口 chrome 下方，不提升到窗口标题栏。
  文件名保留正文字号与未保存标记，关闭为 26px / 6px 圆角图标块，下接 divider 细线。文档不另设底栏：真实行列、问题、自动换行、
  Markdown 即时编辑 / 源码、缩进、编码、换行符和语言统一放入窗口全局状态栏；操作使用紧凑 ghost Button，行列使用等宽数字。
  最小支持窗口内，diff 标题与改动摘要在既有文本区域收缩并省略，保留 Split／Unified、Dock／Fill 和关闭控件的命中范围；不能以文字的固有宽度把这些既有操作挤到窗口外。裁切不改写标题、摘要或 patch 数据。
  `.md`、`.markdown`、`.mdx` 默认进入原生即时编辑，源码切换不创建第二份文档或撤销历史；不保留旧只读预览设置。“新建 Markdown 文档”是独立、无默认快捷键的命令，不替换普通文本新建或终端回滚内容导出。
  Markdown 即时编辑的体验改进以 [Zorite](https://github.com/packetThrower/zorite) 的完整所见即所得编辑体验为对齐目标，不能以只改善部分高频操作替代完整目标。范围限于 lyrel 的文档编辑区域，不因此引入日记库、知识库管理、白板或 PDF 工作流。
  对齐包括文档内部的正文宽度、字体层级、行距、段间距、列表缩进、表格与代码块外观及编辑控件；窗口、终端、文件树、全局状态栏、主题颜色与缩放继续遵循 lyrel 的设计系统。
  体验改进不得隐性规范化未编辑源码；保留同一文档与统一撤销历史、本地／远端 Host 保存语义，以及现有 MDX、公式、Mermaid、frontmatter 和图片能力，不因上游缺少对应能力而移除或降级。可以调整内部实现，但不能通过改写文件或减少既有能力换取表面顺滑。
  此轮保留完整编辑功能目标，按负责人要求聚焦 Windows 核心工作流验证，优先脚本和后台测试，减少前台审批；macOS 保持兼容，未实测部分明确记录，不声称完整原生或跨平台体验通过。核心交付、实际证据、已接受性能成本，以及负责人最新确定的文本排版支持边界和应用层保护，统一见 `docs/window/side-panel.mdx`。
- **SFTP 浏览器**：行与本地文件树一致（26px、圆角 6、16px 图标辅助色、文件名侧栏标题色、大小等宽数字）；
  编辑框为无边框淡底井（圆角 7）；传输进度为正文色细条压在淡底轨道上，失败才转红。
- **面板里的主按钮**（SFTP 确认、添加转发、SSH 断线条的重连）：反色中性，同对话框主按钮。
- **对话框**（SSH 认证/主机密钥、新建 worktree）：`ui::dialog` 统一外壳，同切换器的 12px 圆角、
  浮层阴影与遮罩、距顶 112px；标题行 48px（正文字号 medium，右侧 `esc` 键帽，下接细线），
  内容区左右 18px、字段间距 14px；字段名 11.5/16rem medium 辅助色，输入框为无边框 28px
  淡底井（`muted`、圆角 7）。页脚 40px 细线分隔，按钮 28px / 圆角 6、右对齐、主操作在最右：
  主按钮为反色中性（同提交按钮），次按钮透明、hover 取所在表面的 hover 阶；不可用时实心按钮
  退回淡底 + 辅助色且不响应点击。主机名与指纹放在淡底信息井里用等宽字。唯一的红色按钮是
  「主机密钥已变更」的 Override，该卡片也是唯一带红色描边的对话框。
- **浮动提示条**：边缘保持中性细线，严重程度只由行首 6px 状态点表达（警告琥珀、断开红）。
- **首页**：快捷操作列表 340px 宽，行高 28px、圆角 7，静止无底、hover 取窗口 hover 阶；
  快捷键用 18px 淡底键帽。
- **终端**：ANSI 是用户内容，不拿来充当 UI 错误/成功配色；应用设计调整不得反转状态含义。
  Windows ConPTY 的临时光标位置补偿只影响绘制，不能改写 VT 解析光标或自动折行状态；
  下一段相对输出必须从原始字节流决定的位置继续。流式验收需在最终整屏重绘之前检查相邻内容是否保留。
  预算回收的终端图片在仍保留的原区域内绘制 `Image evicted`／“图片已淘汰”，文案走 `L10nKey` 的英文／简体中文资源，颜色取 `muted_foreground`；沿用图片裁切，不以 ANSI 内容颜色表达应用状态。
  提示是纯绘制元数据，不加入终端文本、选择、行数或 PTY，不弹 Toast，不添加交互。主动删除、clear／reset 与历史过期不显示提示；原生像素、双语和浅深主题对比仍须逐平台验收。
  Unicode 数学括号拼接字符 `U+239B`–`U+23AD` 与框线一样按终端单元格绘制，
  圆括号、方括号和大括号在行高大于字体高度时仍须连接；沿用终端前景色，不改字符、
  选择文本或公式布局。Windows 隔离实窗已通过真实 `omp/18.12.6` 的分式、矩阵、分段函数
  合成会话重放；三行矩阵括号接缝的空白像素行从 6 降为 0。几何回归覆盖多行高与
  1×／1.25×／1.5×／2× 比例，不替代其他 OS DPI、字体和 macOS 的原生像素验收。
  PUA 图标（Powerline 原生分隔符除外）可以向相邻空白借用绘制空间，但两格的显式背景、
  选择状态、普通搜索命中与当前搜索命中必须一致；显式背景还须同色。非空文字、组合标记、
  下划线、删除线或链接装饰占用的空白不能出借。主字体图标和 fallback 图标沿用同一兼容性
  规则，fallback 保留既有 ink fitting；借位只改变 glyph 的绘制预算，不改 Rio 列宽、复制内容、
  鼠标命中或滚动几何。对应语义移植来源为 tty7 `69536b361a2c9ea5711fc779c2bcba3b1efbdbd5`。

  原生回归使用 `node scripts/terminal-icon-overhang.mjs`，在隔离配置和自有 pane 中分别检查
  已安装的 primary Nerd Font 与普通主字体加 icon fallback。保持前七行说明不换行，比较
  default／matching background 行的同一图标，再检查不兼容背景、非空邻格和装饰空白。
  `q` 恢复 alternate screen 并退出，`s` 触发输出与重绘。Windows 可用
  `scripts/measure-terminal-icon-overhang.ps1 -Screenshot <自有窗口PNG> -OriginX <首格X> -OriginY <首格Y> -CellWidth <单格宽> -LineHeight <行高> -VerifyMatching`
  读取所保存 PNG 的坐标并断言 matching background 没有缩小可借位图标；没有实际超单格
  glyph 时脚本明确失败，不把缺前提记为通过。该量尺不验证选择／搜索状态、滚动、命中、
  系统字体 fallback 或 GPU 原始分辨率；这些必须另做真实界面验收并记录平台与 DPI。

### 终端帧绘制所有权

`TerminalElement` 保留唯一的 `request_layout` → `prepaint` → `paint` 管线与命中几何，
`TerminalView` 继续持有每个 pane 的 `grid_buf`、`grid_snap`、字体、焦点与终端连接；
私有绘制模块不创建第二个 Entity、帧缓存或锁，也不接管终端生命周期。

- `element/snapshot.rs` 在同一次 grid snapshot 中解析 cell、选择、搜索与链接标记；
  稳态继续使用 `try_lock`，无法取得 grid 时不修改旧 buffer，由管线重画上一有效帧。
  只有首帧或普通 resize 后旧帧行列不匹配时保留阻塞获取例外；不能只比较单元格总数。
  同步 hold 或竞争时复用的 committed cells 沿用原行列 addressing，即使新旧尺寸等面积也不重新解释；
  已持有的同步帧在有序 `Size` 到达前只受新边界裁切，不等待实时 grid；ESU 或期限先结束 hold
  也不代表 resize 已生效。锁内 VT 行列尚未匹配时独立等待 `Size` 的既有 Wakeup，不启动竞争重试。
  relink 后没有 previous snapshot 的首个 hold 按新 surface 分配空帧，不恢复上一连接残留的几何。
- `TerminalView` 持有的 `FrameFacts` 统一提供绘制模式、选择、光标及 prompt／菜单几何；首帧先取得有效事实，
  后续在同一 guard 内应用 scrollbar 的待处理位置并采样。稳态 render／paint 不为这些投影等待 PTY reader。
  facts、composer、输入维护、同尺寸 resize 核验或实际 grid 构建因锁忙而跳过时，共用每 pane 唯一的后台等待任务；
  grid 可用后经 `TerminalLifecycle::notify_output` 请求重绘，不轮询帧、不等待后续输出，也不唤醒隐藏 pane。
  同尺寸请求实际校正过 grid 后再绘制一帧，使帧首事实与校正后的几何重新一致；尺寸和 cell metrics 未变且 grid 已匹配时不重发 resize。
  提示符所有权同步、settle／flush 的判断与非阻塞 writer 入队使用交接点同一个实时 grid guard，
  不根据缓存绘制模式向 shell 交接输入；实际按键、鼠标协议及提示符输入路由仍读取实时终端状态。
  同步期间的选择与复制读取 committed cells：Tab 只跳过实际 tab stop 之前的 NUL 展开空白，
  stop 之后的真实 NUL 间隔仍为空格；尾空行／全空选区不凭空加入换行，内部空行与 soft-wrap 间隔保留，
  按行选择单独保留一个末尾换行。普通可达边界以正常 Rio 复制与独立可见字面量共同核验。
  已知例外：正式 Rio `0.5.28`／`0.5.29` 的 normal copy 在 Simple 选区仅终止于 `LeadingSpacer` 时，
  上游补偿分支会读入前一物理行的首字符；本次不改 normal 路径，也不将该旧 quirk 复制进 held 路径。
  held 只序列化 committed 实际可见选区，不引入前行字符或未选中的下一行 wide base，不宣称所有边界字节无例外等价。
  GUI 通过原有 `ParseBridge` 回调记录一维 stop 元数据，涵盖 HTS／TBC／RIS 与 resize 后新增的默认 stops，
  复用原有光标元数据 mutex 并在同一 VT guard 内采样到 cell 的 bool；不克隆整表、不建立第二个 grid／解析器或新锁。
  已进入 presented 状态的拖选更新不获取实时 grid 锁；只在采用或更新 live selection 时取原有锁。
- `element/colors.rs` 负责 ANSI／truecolor、主题颜色与 pane dim 的解析；
  cell 样式复用只属于当前 snapshot，不跨调色板或主题变化保留解析结果。
- `element/text.rs` 与其私有 `segment`、`powerline` 模块负责字体、Unicode cluster、
  宽字符、组合字符、fallback ink fitting 与原生分隔符；单元格列位置不变，glyph 裁切遵循上面的兼容空白借位规则。
- `element/decorations.rs` 绘制背景、选择／搜索填充、特殊下划线、cursor 与 IME preedit；
  所有阶段借用同一 buffer，不能为了模块交接增加每帧 clone、allocation 或 lock。

图片继续由既有 image store 拥有资源。管线保留图片低层、cell 背景与选择／搜索、
负 z 图片、文字与下划线、非负 z 图片、cursor／IME、bell 的绘制顺序；
图片的 claim、register、atlas 释放时机及 pane clipping 不移入独立状态所有者。
绘制回归按颜色、snapshot、文字与装饰分组，小型 fixture 位于 `element/test_support.rs`；
真实终端与 graphics consumer 继续从既有窗口 seam 验证。

职责迁移后的验收须分别记录自动化结果与隔离原生窗口中的 Unicode、选择／cursor、
图片、resize、持续输出及 grid 竞争 fallback 证据；本节不把此前的视觉记录当成本次结果。


### 框架滚动约束

`gpui-kit 0.8.0` 的 `List`、`Editor`、`Textarea` 等框架组件采用官方滚动语义：macOS 的精确滚动默认回弹，普通行滚轮不回弹。已有自绘侧栏、文件树和 Git 面板不为统一回弹而改写。

指针下最内层可滚动输入框或编辑器在到达边缘后仍保留手势；内容放得下及单行输入框的垂直滚动仍可交给外层。关闭 `scroll_bounce` 只取消回弹，不能恢复旧版的父级接管行为。不要增加全局滚轮拦截来复刻旧交互；普通滚轮驱动横向 strip 的边缘接管是独立规则，见 [0.8.0 发布说明](https://github.com/longbridge/gpui-kit/releases/tag/v0.8.0)。

## 验证要求

`cargo test -p lyrel --lib --locked ui::` 覆盖内置主题的文字、语义色、
交互状态对比度及已有 UI 行为。另需在真实窗口检查浅色/深色、宽/窄窗口、侧栏、文件、
Git、设置、菜单与随处搜索。测试通过不能替代视觉层级检查。

文档栏几何验收须经过生产窗口与文档消费者，而不只断言宽度计算函数：分别测量文档列、其内容盒与编辑器实际输入区域，确认内容铺满列内空间且右缘不越界；在窗口 resize、界面字号变化和 `1×`／`1.25×`／`1.5×`／`1.75×`／`2×` display scale 下检查。通过既有 Dock／Fill 与 diff 的 Split／Unified 控件往返切换，核对两组控件的垂直中心、未保存正文、焦点与 patch 状态不丢失；窄窗口临时填充后放宽应按原选择恢复。原生 Windows 与 macOS 的窗口 chrome、交通灯、全屏和实际 DPI 必须分别记录；GPUI test-platform 的 scale 模拟不是原生多 DPI 或 macOS 验收。

0.8.0 的 Windows 实窗检查使用隔离配置与临时工作区，覆盖 Unicode 编辑保存、长文件查找与滚动、真实 `rust-analyzer` 补全候选与 Enter 接受、多行输入框增长、菜单、命令搜索、浅深主题、文档分隔线和宽窄布局。macOS 的触控板回弹、动量、原生焦点与渲染仍未验收；Windows 证据不能替代。Unicode 文本注入也不能替代真实中文 IME 预编辑与候选窗验收。

Windows 终端回归在隔离配置实窗验证：`omp 18.12.4` 原生 `askDialog` 的方向键选择与提交、
退出后恢复 shell 历史、`Ctrl+L` 后重启 GUI／daemon 不复活已退出会话，以及本地 ConPTY
分帧输出暂停时三行内容同时保留。Ask 通过临时扩展直接打开原生组件，未调用模型；
流式场景使用确定性 PTY fixture，不代表已覆盖所有真实模型输出、macOS 或远端 SSH 渲染。

### #172：Kit／Fast／Rio 组合的 Windows 验收记录

本记录区分已通过的具体路径、被工具或环境阻塞的项目与尚未验证的范围，**不表示完整 Windows 或跨平台验收已通过**。维护者决定本轮以实现交付收尾，#168／#172 随 PR 合并关闭；下述未验证项保留原状态，macOS 验收集中保留在 #167。
原始证据保存在验收机外部目录 `C:/Users/dlivx/AppData/Local/Temp/lyrel-168/`，以下短路径均相对于该目录，不随仓库分发。

#### 候选、环境与隔离

| 身份 | 实际记录 |
| --- | --- |
| 升级前 baseline | `069eade7bdd31e19f5381895c805ed962d767fdb`；Kit `0.8.1`／Pre `0.3.10`／Rio `0.5.28` |
| 最终生产候选 | `ead83a2c483ae1a661a5bb8bcd23b8a62858245d`；Kit `=0.9.0`，仅由其 `gpui-fast` feature 选择 Fast／platform／Windows `0.1.13`，HTTP transport `=0.3.11`；三个 direct Rio 与 grapheme-width 为 `0.5.29`，VT defaults 仍关闭，正式 registry-only |
| baseline lock SHA-256 | `80e68ed9886f3718a51449bddd4936a9917d33945d5c2d13a2e0bdff5b20ea53`（保留的 `baseline-Cargo.lock`） |
| candidate lock SHA-256 | `1fd42a8afe7f62d8545e95a59ca1d9ff6b079a78746ecfe668322f7dcd3c4387` |
| baseline GUI | `baseline-bin/lyrel-app.exe`，148,054,016 bytes，SHA-256 `88beec96dc9bf6fd8e4c5955e92fda9fc7786c2afa4285f58befedf96fcb1e5f` |
| candidate GUI | `reviewed-final-bin/lyrel-app.exe`，153,307,136 bytes，SHA-256 `d9af8a843f0772457ac3ee7e01c5e0a792c8ef955503247d997aabd88274ef9e`；CLI／server／updater／ConPTY／许可证完整身份见 `reviewed-final-windows-build-identity.json` |
| 验收机 | Windows 11 Pro for Workstations `10.0.26300`、Core Ultra 7 265KF（20 核／20 线程）、RTX 5080／驱动 `32.0.16.2101`、约 32 GiB RAM、卓越性能；Rust／Cargo `1.99.1`、原生 Node `26.13.0` |
| 显示与条件 | Q27G4Z `2560×1440`、scale `1`；Release、实际 Light、119×35、terminal font `14`／UI font `16`，可见且不最小化，每轮采样前附着 AX；无 Cargo／测试／其他性能 cohort 并行 |

最终候选包含 Explorer 隔离修复 `c51d3e8`、native SSH fixture 安全补丁 `5df2f28` 与后续 GUI reviewfix。
`0e7ddc0`、`5df2f28`／`31712d8` 的旧 GUI／性能结果只作历史检查点，不替代最终候选；被取消的旧候选任务不算通过。
Linux headless／SSH 与 musl 产物分别保留自己的 source commit，后续 GUI-only 修复未改变其 core／CLI／server／lock，不把这种源码等同性写成另一次实测。

生产 GUI 只使用新建自有 root，关闭 `install_cli_on_path`、`check_for_updates`、`auto_download_updates`，重定向平台与 agent 配置命名空间并清除继承端点；没有操作普通用户 GUI／pane、修改账号或全局配置。
`native-registry-isolation-comparison.json` 证明含 c51 的 production5df 实窗前后两项真实 HKCU Explorer 键、递归子键、全部值及原生 LastWriteFileTime 完全相同，前后 SHA-256 均为 `603e7a8e969c5ba66f2e2fba221c4edefc5b45e12a34edddeed91ed5a8e28361`；`native-user-path-comparison.json` 证明用户 PATH 原始值种类与哈希相同。全程只读比较，没有恢复注册项或泄露 PATH 内容；该中央隔离函数不受后续 GUI reviewfix 改变。

#### 构建、测试与包：已通过的范围

最终 `ead83a2` 实际执行以下命令，进程级清空 `RUSTC_WRAPPER`／`CARGO_BUILD_RUSTC_WRAPPER`、使用已核验的自有 ConPTY，未覆盖仓库 `+crt-static`：

```powershell
cargo fmt --check
cargo test --locked --workspace --all-features -j1
cargo build --release --locked --features updater -j1
```

三项 exit 均为 `0`。标准顶层测试 **5166 passed／0 failed／56 ignored**，另 CLI E2E **26 passed**；updater 的 30 项已包含在 5166 中，8 个嵌套 helper 不重复计数。
完整 ignored 名单、Windows 上的 Unix-gated 0-test suites 与既有 warning 保留于 `windows-reviewed-ead83a2-allfeatures-release.log`；ignored 和 0-test 不算通过。
复合检查／测试／Release 总耗时 1470.66 秒，不冒充纯 Release 耗时。精确结果见 `reviewed-final-windows-test-summary.json`。

专用可丢弃 `wt-package168` 使用冻结 Windows binaries 与真实静态 musl server 执行现有 `bundle-windows.ps1 x86_64-pc-windows-msvc x86_64`、`verify-windows-package.ps1 x86_64`、`assert-no-vcruntime.ps1`，exit0；版本 `0.3.0`、x86_64、ZIP／Inno payload 布局及 GUI／CLI／updater 的 VC++ runtime import 检查通过。Microsoft 预编译 ConPTY 按脚本规定跳过自有链接检查，但包内版本、哈希与许可证独立核验。
完整命令输出为 `reviewed-windows-package.log`，产物来源为 `package-reviewed-stage-identity.json`。没有运行安装器、系统注册、签名配置或发布；包生成／检查不等于实际安装验收。
本轮保留的 ZIP 为 59,353,384 bytes、SHA-256 `f74049de23ec0328fa4443038182214938c1f1e392d98c298e1c4644b8996712`；setup.exe 为 41,502,033 bytes、SHA-256 `a9ae2cfaf8448f28fb0f06d77bef1cbff3d710213fb761e28ef41deb01c4ec1e`。这两个本地验收包未发布。

musl server 来源 `90e8b7dded8702024a8fbc52e6a351e2d243db73`，与最终候选的 headless 源码／lock 相同；以 CI 的 `cargo zigbuild --release --locked -p lyrel-server --target x86_64-unknown-linux-musl -j1` 和 SIMD flag 实际构建，exit0，319.96 秒，`assert-static` exit0（无 PT_INTERP／DT_NEEDED／dynamic section）。14,665,264-byte 产物 SHA-256 为 `a8c1e6f4b5435853f7635e73e956a00bf412f50add2f6356c2fef3be576c26a8`；Zig linker warnings 与首轮 unsupported `--version` 前提失败保留在 `reviewed-musl-build-identity.json` 指向的 raw 目录，不用 GNU server 冒充 musl。

#### 原生交互：实际通过与证据边界

`native-reviewed-workflows/observations.json`、同目录 PNG／AX 与 `lifecycle.json` 记录最终 Release 的具体结果：

| 路径 | 实际结果 |
| --- | --- |
| 启动、文字与 PTY | 原生 ConPTY 显示中文、宽字、组合字符；逐字节 UTF-8 分片保留中间与完成画面；alternate-screen TUI 显示后恢复原主屏，未只检查最终 repaint |
| 可见选择／复制 | 终端实际双击选择，通过搜索命令 Copy＋Enter 得到与可见 `NATIVE_COPY_MARKER` 相同的剪贴板；生产编辑器用实际右键菜单 Select All／Copy，核对自有文档。不是只调用 action 或注入 `Ctrl+C` |
| 同步呈现 | CLI 已包含 `PARTIAL_NATIVE_HIDDEN` 时，GUI 在相同 viewport 的真实 Search 浮层重绘下仍保留 committed 文字；结束后显示 `ATOMIC_NATIVE_END`。quiet 无后续输出时约 300 ms 截图已显示新文字；截图只给完成上界，精确 150 ms 期限另由真实 Root／reader 消费者回归证明，不是物理延迟测量 |
| 编辑器／Markdown／Diff | 实际输入、菜单选区复制、Wrap:on、查找 `1/1`、自有文件保存；代表性 Markdown 表格／Mermaid、live／source 同文档与真实 Git unified Diff 保留像素／AX。没有把普通 Unicode 输入记作 IME |
| 菜单与焦点 | 实际 Search／Copy 命令、编辑器 context menu 及 unsaved Save 对话框有可观察结果；不扩大为所有嵌套弹层、通知复制和快捷键／焦点恢复全通过 |
| 主题、字号与布局 | Light／Dark／自有 `acceptance-midnight` YAML，UI font 18／terminal font 16、两侧栏拖拽与折叠、F11 往返、原生最大化／还原均有实窗证据；只测 scale1，不等于多 DPI 或所有透明材质通过 |
| GUI 生命周期 | GUI 正常关闭 exit0 后 daemon 仍为 pid75540、原 Node PTY 仍为 pid73704；重新打开附着同一 daemon 与同一 PTY，未通过新建 shell 冒充恢复 |

等面积 resize、ESU／timeout 早于 ordered Size、relink 首个 hold、Tab stops／Lines／空行／Unicode 可见复制、presented 拖选持锁完成性，另由既有生产 Root／TerminalView seam 完成 red→green；见 `handoff-review168.md`。
最终完整日志明确包含 `gui_and_cli_shared_synchronized_ranges_converge_at_the_same_geometry`、`graphics_consumer_hidden_retired_atlas_does_not_wake_window_and_returns_current_pixels` 等通过结果，分别证明同几何收敛、隐藏返回当前像素与无多余窗口唤醒。可见图片删除／恢复和缓存失效消费者结果与实窗结果分别记录，不能把自动化 paint／计数断言扩大成原生 GPU、任意 hidden-pane 场景或物理输入延迟证明。

Windows 到 owned Linux 42368／42369 的真实 SSH transport／Host 图形 5 项与 lifecycle 1 项已在 `31712d8` test exe 显式执行，全部 exit0，包括 nested SSH、不 skip；日志为 `windows-ssh-317/{graphics,lifecycle}.{log,err}` 与 `result.json`。后续 GUI-only 修复未改变该 core／server 路径。此处证明真实 Windows SSH，不代替最终生产 GUI 远程工作台的实窗结果。

最终生产 GUI 的真实远程工作台另见 `native-reviewed-ssh/native-ssh147-aikhtS/observations.json`、PNG／AX、session／sample：固定 host-key pin 的 SSH 连接到 Linux daemon79893，实际 sender PTY 为 99059／99060，两个 `TerminalView` 独立显示左绿／右蓝，再均变蓝；新 GUI 进程恢复同一 daemon 与相同 sender PID，保留蓝色图片。远端文件树展开、远端中文 Markdown 表格读取通过，owned fixture 已停止。
远端编辑输入／保存**未通过验收**：请求 `SSH_NATIVE_EDIT `，实际只观察到 `H_NATIVE_EDIT`；自动 takeover 后焦点归还未确认，AX／像素关闭尝试没有产生保存确认，草稿仅保存在证据目录。工具 delivery 问题已报告，现有证据不足以归因于生产回归，也不足以宣称无输入丢失；不把远端读取或 local 保存冒充远端写入通过。

#### 原生材质：受控像素与往返

最终 `ead83a2` 以自有 16px 黑白 checker 和全黑／全白背景，分别检查 Light 与通过真实 palette 选择的 Dark。
`native-reviewed-materials/observations.json`、`captures-manifest.json`、`checker-stats.json` 与 `material-reviewed-final/{opaque-off,light-opacity-088-off}/pixels.json` 保留合成像素：

- opaque／Off 时，背景 marker 黑白差为 255，两侧栏及终端空白 patch 的差均为 0；opacity0.88／Off 时 rail／terminal／sidebar 分别为 30／31／30，逐 RGB 与升级前 Pre0.3.10、Kit／Pre0.3.11、Fast 早期检查点相同。这里只证明保留原外观，不重新宣称旧文档的 85% 透出目标已实现。
- 同一 96×96 终端空白 ROI 的 checker 标准差在 Light、Dark 的 Off 下均为 15.5，Blur 下均约为 0.302786；Off→Blur→Mica→Acrylic→Off 往返后恢复相同 Off mean／std。Mica／Acrylic 为恒定 patch，不能据此把 Mica 称为实时桌面 blur。
- 最初五张文件名虽含 `dark`，实际为 Light；真正 Dark 是之后经原生 palette 选中后生成的 `verified-dark` 5–9，身份按 manifest／stats，而非文件名推定。字号始终保持 terminal14／UI16，结束恢复 opacity1／Auto；GUI 与背景进程均 exit0，无剩余本轮 GUI。

上述结果只覆盖已测 scale1、主题与材质往返，不扩大为全部 DPI／激活状态、任意背景对比度或 macOS；完整 daemon／工作树收尾由父会话单独管理，不凭 GUI 退出推定全部资源已清理。


#### 同条件 Release 性能

`native-reviewed-perf-{0,1,2}/` 每轮保留独立 40 秒 load／20 秒 idle、producer raw 与像素；汇总见 `native-reviewed-performance-summary.json`、`reviewed-windows-performance-comparison.json`。
每轮前后 `reviewed-performance-display-guards.json` 均确认 Q27G4Z／scale1；中途另有显示变化的记录未混入此批样本。旧 baseline 因已知 Explorer 标签写入风险不再重启，使用保留的匹配采样，不声称新做了交错 A/B。

| 指标（GUI＋daemon 内存／CPU） | baseline median | candidate median | 成本变化 | 独立样本数 baseline／candidate |
| --- | ---: | ---: | ---: | ---: |
| producer drain，ms | 116.7022 | 118.5506 | +1.584% | 5／15 |
| redraw workload elapsed，ms | 4843.6184 | 4852.0646 | +0.174% | 5／15 |
| cold working set，bytes | 147705856 | 152086528 | +2.966% | 3／3 |
| cold private，bytes | 140646400 | 137811968 | −2.015% | 3／3 |
| load working set，bytes | 180166656 | 185876480 | +3.169% | 1／3 cohorts |
| load private，bytes | 175075328 | 172591104 | −1.419% | 1／3 cohorts |
| load CPU，CPU 秒／墙钟秒 | 0.091638 | 0.092809 | +1.278% | 1／3 cohorts |
| idle CPU，CPU 秒／墙钟秒 | 0.017006 | 0.013913 | 绝对量评估 | 3／3 cohorts |

已测成本／内存 median 均未出现大于 5% 的退化；drain baseline 范围 114.9517–120.1379 ms、candidate 114.4956–124.3944 ms，redraw 分别为 4532.3557–5030.0253 ms 与 4686.0219–5100.1093 ms，完整 spread 保留，不挑最快样本。
idle CPU baseline 范围 0.014704–0.024773、candidate 0.010045–0.016247 CPU 秒／墙钟秒，按绝对用量和静态重复采样评估，不放大近零相对比例。load 基线只有一个独立 cohort，不能据此宣称该项已完成同等强度的重复噪声判别。
producer drain／workload elapsed／FPS 不等于 CPU paint、GPU frame time 或物理显示延迟；没有保证提速，也没有 input-to-photon 数据。构建时间与 exe 尺寸单独报告，cache footprint 未测，不作为运行时改善证据。

#### 仍阻塞或未验证

- **blocked：**真实 IME preedit／commit／candidate 没有可证明的后台路线；不主动申请前台，不用 Unicode 注入替代。隔离实例明确显示 `rust-analyzer not installed`，未出现补全候选；系统 shim 的版本输出不能替代该实例可用的 LSP，没有为验收安装工具。
- **工具路线限制：**旧阶段后台 `Ctrl+End` 仅执行 End、`Ctrl+C` 插入 `c`，不算快捷键成功；后来通过真实菜单证明选择／复制。自绘字段的 `el.type`、`el.press` 曾自动 fallback 到工具许可的 takeover，Ctrl+F／Ctrl+S 的焦点归还未确认，不称全程纯后台或快捷键／焦点恢复通过；没有主动 `raise`／申请前台控制，失败路线和用户剪贴板恢复记录保留。
- **unverified：**多 DPI、未测的材质激活状态／任意桌面对比度、所有嵌套对话框／通知交互及焦点恢复、物理输入／显示延迟、实际安装／系统注册／发布；不能把本轮有限像素矩阵或其他平台证据推广为完整通过。
- macOS SDK／实窗／IME／Spaces／Retina／性能仍由 #167 真实 Mac 验收；Linux #173 独立记录，不因 Windows build 通过而推定 Unix 已验收。工单关闭仅表示维护者接受本轮实现交付，不将上述 blocked／unverified 项改记为 passed；任何正确性回归无论百分比均阻断对应验收。


### 根布局与窗口控件所有权

`LyrelApp::render` 保留窗口级命令、设置入口、认证提示、工作区切换器与随处搜索；
`render_work_area` 是终端、文档、右侧面板及内容区拖放反馈的唯一组合入口。
全局状态栏与 `workbench-content` 是纵向布局兄弟；内容容器继续用 flex 填满余量并独立裁切，
不在标签导航中扣减高度，也不靠状态栏覆盖溢出。

`src/ui/window_chrome.rs` 独立拥有完整透明顶栏、平台原生窗口控件与左右常驻三按钮组；顶栏挂载和几何布局不由侧栏显隐或宽度决定。品牌始终保留，侧栏收起时不生成迷你轨道。`tab_sidebar.rs` 只拥有唯一左侧标签导航、整宽真实工作区切换器、分组、滚动与宽度拖拽；`tab_strip.rs` 保留共享标签呈现与菜单，不再渲染横向标签。
横向模式、`ToggleTabSidebar` 和悬停隐藏 chrome 均已退役；配置升级仅删除两个失效键，不重置其他偏好。Settings 只移除两项失效设置，不重构其信息架构。
右面板页签独立位于窗口按钮组下方；macOS 窗口模式保留原生交通灯，macOS 与 Windows 全屏均使用没有失效原生按钮的同高拖动行。文档铺满被限制在 chrome 下方，不遮挡搜索或布局操作。

窗口 chrome 的唯一实现所有者是 `src/ui/window_chrome.rs`：`WindowChrome` 组合常驻按钮与平台原生控件，接收左右面板可见状态、分屏可用性、新标签页菜单元素及 `ChromeCommand` 监听器工厂，不读取或修改整个 `LyrelApp`。工作台只在挂载点处理面板、搜索、分屏及最终焦点交接；新标签页菜单的业务仍属于标签导航。

共享标题行的高度、底边预算与垂直中心、tile 尺寸与样式、品牌图形、交通灯位置和原生按钮可见性均从 `window_chrome` 消费，不经 `app` 或 `tab_strip` 重导出。设置窗口、文档标题、diff、右面板、Git、GitHub、文件树及 SFTP 共用该接口；颜色仍来自既有主题角色，尺寸和平台例外保持原样。

窗口拖动的 armed 状态由 chrome 私有实现持有，按每个窗口内稳定的拖动行 key 保存在 GPUI keyed state 中；重绘不得重新创建手势状态。单独按下不移动，首次移动只请求一次原生拖动，松开或在其他区域按下解除 armed；resize 手柄与按钮组继续通过阻挡命中避免误触拖动。

chrome 的回归测试集中在 `window_chrome` 的几何、真实工作台消费者和拖动手势测试组，复用既有 `app::test_window` fixture。界面缩放／全屏往返测试通过真实按钮操作检查命中尺寸、固定顶栏、终端顶部与退出全屏后的占位恢复；按下后重绘再移动、单独按下、松开后悬停及 resize 命中测试保留原生拖动请求语义。这些测试不能替代各平台实际窗口系统的拖动和原生控件验收；职责迁移后的测试与实窗结果须按当前提交单独记录，不能沿用下述历史验收作为通过证据。

左右按钮组必须阻断底层 `TitleBar` 的命中与按下事件，不能只在 `on_click` 中阻止冒泡。按钮按下后发生轻微指针移动仍须激活对应命令；只有按钮组外的空白区域可以进入窗口拖动。原生实窗验收需包含按下、组内轻微移动、松开的完整手势，不能只验证静止点击或直接调用命令。

固定顶栏已在 Windows 隔离实窗验证四种侧栏状态、窄窗口、默认及放大界面字号、浅深主题、透明配置、全屏往返与真实终端分屏；折叠往返保持终端顶部和 PTY 行数，按钮支持按下后轻微移动再松开。透明配置的窗口捕获只用于布局验收，不替代原生透明合成的像素差分。

macOS 已在隔离实窗以默认界面字号、浅色主题验证全屏两端边距、四种侧栏状态、退出／重新进入全屏、真实终端分屏与搜索，以及按钮按下后轻微移动再松开的手势；非全屏边距与交通灯布局保持原样。GPUI 布局回归另覆盖 760px／1440px 宽度与 16px／20px 界面字号。此证据不扩大为 macOS 深色、透明合成或其他界面的完整验收。

文档停靠时使用列内标题，只挂载一个文档内容；铺满时保留浮层顺序。组合过程复用已有 `Entity`、连接与焦点句柄，不创建会话、不转移持久状态。关闭随处搜索恢复此前的终端或文档焦点；关闭文档或面板后的焦点恢复仍由原命令处理。

随处搜索的目录与生命周期由既有 `src/ui/search/` 拥有：`catalog` 根据工作台提供的
chrome、编辑器命令可用性、共享 `TabRow` 与 Host 资料构建动作、终端、文件和会话目录；
`history` 统一负责按 Host 缓存、OpenCode／Hermes 查询上下文、结果去重、排序与当前目录分组。
`SearchState` 持有当前 `SearchView`、事件订阅和打开前的焦点；关闭、替换或切换工作区时先退役旧视图，
旧视图即使仍被持有也不能接收迟到的历史查询，旧事件不能关闭或导航新的搜索。工作台仅执行既有
`SearchEvent`／`CommandKind` 语义操作及主题、符号预览的最终焦点交接，不复制设置窗口的 bridge。
键盘入口继续返回原终端／文档焦点；指针入口保留打开前实际取得焦点的控件（包括 toolbar 按钮），
不将它强行改为终端焦点。此所有权迁移的测试、格式检查与原生 keyboard open／select／dismiss／focus
smoke 由集成分支集中执行，不能以既有布局验收记录代替本次结果。

独立设置窗口由 `SettingsWindow` 持有输入、草稿、快捷键录制、订阅和页面异步任务；
`src/ui/settings/` 按配置应用、主题编辑、导航与输入等职责组织实现。工作区不再持有这些编辑状态。
`src/ui/app/settings_bridge.rs` 仅提供按需只读工作区资料，并处理连接 SSH、打开文件、重启 daemon、
配置生效及关闭设置等明确事件；pane 树、文件打开与工作区焦点仍由 `LyrelApp` 管理。
关闭与重新打开保留原分区和滚动位置；未保存草稿仍先确认，迟到任务不能写回新的设置会话。

全局状态栏默认高 `1.625rem`，最左侧图标打开既有独立设置窗口。主机入口打开工作区切换器；
路径跟随当前终端或文档并可复制，远端路径不按本机平台改写；分支及 Agent 状态来自实际会话。
时钟以本机时间每分钟更新独立实体，不驱动终端输出重绘。中间默认留空，不播报标签创建或切换。
长路径与分支允许收缩；设置、时钟及后续全局操作保留自身空间。

终端工作流的 CPU／MEM 是工作区所属整台主机的内置指标，不是 GUI 进程或窗格进程树。
本地窗口经已有 control 链路请求 daemon，由 daemon 的原生 `Host` 采样；CPU 为所有逻辑核归一化的
0–100% 忙碌率，MEM 为系统定义的已用物理内存比例，不含 swap。提示中显示实际主机名与已用／总 GiB。
首轮 CPU 只建立基线，显示等待而不是 0%；不可用、断开和采样失败保留文字状态与占位，不显示伪造零值。
远端工作区经该主机既有 `RemoteHost`／control 链路请求远端 daemon 的同一原生采样器，不在桌面执行远端路径或采样本机。
连接中、重连中、断开、缺少指标能力及采样失败分别显示明确状态。按 `HostId` 共享视图并验证 control hello 的 daemon instance；
切换主机同步替换视图，daemon instance 改变后拒绝旧采样。同一 daemon 重建 control 连接时成功采样仍有效，
但旧连接的错误不能覆盖新连接状态；不读取本机来冒充远端，也不复用另一工作区主机的样本。

daemon 的采样按主机共享，最多每秒刷新一次；没有请求时不采样，空闲超过十秒后重新建立 CPU 基线。
系统调用与阻塞 IPC 经 `HostOps` 共享线程池执行；GUI 的独立指标实体按主机在多窗口间共享，离开该主机最后一个终端工作流后停止轮询。
文档工作流使用同一前景状态判定替换指标，不能由 PiP 或窗格焦点选择另一台机器。
control 新消息与共享窗格呈现同属未发布的 `CONTROL_VERSION=16`，pane 协议不变。
本地原生采样具备 Windows、macOS 和 Linux server 路线；各平台实际结果仍须单独验收，不能以编译代替采样或实窗证据。

窗格手柄的顶部感应区高 `2.25rem`，叠加在内容内，不设置永久的终端顶内边距。
静止时隐藏；指针经过感应区、键盘聚焦或菜单打开时显现。标准 Button 和 DropdownMenu
负责命名、激活、菜单导航与关闭；shell 的 Tab 绑定不被窗口导航替换。

文档状态跟随当前前景内容，而不是只检查代码面板是否已打开。停靠文档旁的终端取得焦点后，
状态栏恢复终端路径、分支与 Agent；回到文档时恢复当前编辑组的真实信息。差异视图显示自己的 Host/path，
不借用被盖住编辑器的行列与格式。填满文档时保持文档上下文；设置、菜单等临时焦点不改变最后的内容上下文。
所有文档状态操作仍作用于当前 buffer，并沿 buffer 自己的 Host 保存；远端路径不走本机路径转换。
行列入口打开既有跳转框，Escape 只关跳转框并回到文档；问题、预览、换行与换行符操作完成后也回到文档。
窄窗时文档状态区可横向滚动，不挤掉设置、时钟与全局通知入口；文档与全局状态不重复显示。

### 应用通知历史

状态栏最右侧的铃铛打开原生通知收件箱；所有主窗口及独立设置窗口收到的应用 Toast 共用本机历史。
成功、信息、警告、错误均通过 `ui::notification_history::NotificationWindowExt::notify_app` 接收，
保留原来的短暂 Toast、重试与关闭行为。界面显示实际时间、严重程度、简短内容及可用的工作区／标签页／pane 来源。
来源仍存在时可跳转；停放来源只打开对应卡片预览，不自动恢复共享布局，也不给隐藏终端输入焦点。
来源消失时保留记录并明确禁用跳转，不重建会话。严重程度始终有文字，不能只靠颜色。

收件箱只保留全应用最新 500 条，在本机配置目录的 `notifications.json` 保存记录和未读状态；
打开时将当前记录标为已读，各窗口同步计数。列表使用原生虚拟滚动，浮层随窗口与界面缩放收缩，
Escape／外部点击关闭并由原生 Popover 归还焦点。这里不是操作系统通知历史、终端输出归档或远端同步功能，
也不会补造 GUI 关闭期间未收到的事件。

明确标注「清空通知历史」的按钮一次清空所有本机窗口的记录及未读数，无确认，也不发送清空成功 Toast。
事件循环中的 clear、append、已读和来源补齐操作按接收顺序进入单个持久化 writer；写盘结果不回写旧内存快照，
延迟来源补齐也不能复活已清空或淘汰的记录。清空之后收到的真实通知正常保留；退出 GUI 时等待队列写完。

### #22 工作台原生实现约束

前置工单 #23 已在 Windows 隔离原生样例验证卡片变换与整窗材质路线；生产工作台仍须独立验收。
公开 `Window::paint_svg`、`TransformationMatrix` 可承载 Latin/CJK 整卡旋转和无 perspective 的翻面；
每个重叠 sprite 使用独立绘制层，命中逆变换与绘制共享几何及内容裁切边界。
状态栏必须是内容区的布局兄弟，卡片、阴影、光环和命中区不得进入状态栏。

原 HTML 的 56×74 背面因 flex-shrink 裁字；用户已批准仅将选中预览扩大到 120×112 设计单位，
保留静止牌堆、扇形、单牌抬升/拉直及翻面。上述尺寸随界面 rem 缩放，不是物理像素常量。
普通停放使用独立交互暖色角色，不复用 warning；恢复按钮使用成对的主色/前景色。
减少动态默认服从 `gpui_kit::init` 读取的系统偏好，不用启动默认 false 覆盖它。

侧栏与终端共享一个原生窗口材质，侧栏 tint 不是独立 blur。Windows DWM/WCA 与 macOS 原生合成
不承诺浏览器模糊半径或饱和度的逐像素等价。用户明确豁免 #22／#23 的 macOS 实窗闸门；
macOS 渲染、焦点、Spaces、Retina 和 IME 仍为未验证，不由 Windows 结果替代。

### 标签页内的画中画呈现

`Tab.presentation` 与完整逻辑 `root` 分开保存：画中画不从所有权树删叶，也不被同步差分解释为
`PaneClose`。分屏只绘制可见投影，画中画挂载同一个 `TerminalView`，不能创建第二个 PTY 或同时
挂载两份终端。单窗格画中画允许中央分屏区为空，不自动新建 shell。

画中画初始位于终端内容区右下角，顶部保留真实标题、直接停放与直接还原按钮。标题区域可拖动，
右下手柄调整同一个终端的大小；指针操作不转移终端焦点。完整外框、阴影及命中区预留内容内边距，
不得侵入兄弟状态栏。最小尺寸为 `20rem × 12rem`，空间不足时以当前内容区为上限。侧栏、窗口与界面缩放
变化后重新解析，不保存裁切后的窗口像素。直接按钮和菜单均操作对应真实窗格，不改变其他 PiP／停放项。

每标签页最多一个画中画；替换前先恢复旧目标。持久化保存目标身份、原分屏 sibling／轴／比例／侧，以及
独立的几何意图：位置是可移动范围内的比例，尺寸是可用内容区域的比例；不同客户端各自按当前窗口和 rem
解析。恢复保留期间的布局编辑，原关系失效时落在当前分屏旁或填满空分屏区。GUI 快照重建与 mutation replay
均保留这些信息，不保存整页旧布局来回滚；原生 SSH 已有视图仍须复用。

菜单、全局搜索与用户快捷键使用同一组 Action：`TogglePanePip`、`RestorePanePip`。移动及调整大小手柄可由
窗口控件的 Tab 导航到达，方向键调整，Enter／Escape 回到原终端。焦点、hover、pointer capture 与未提交手势
属于客户端；手势释放才将几何意图写入 daemon。远端更新不能覆盖当前手势或请求键盘焦点；切换标签页取消
本地手势。真实输入、选择、滚动及跨窗口尺寸的验收须在隔离原生窗口执行，消费者回归与构建不能替代实窗证据。

远端工作区内额外打开的客户端本地原生 SSH 仍由本机 daemon 拥有，不迁移到远端机器树。
混合布局使用 `ScopedPane` 区分 workspace 与 client-local 所有权，本机 daemon 的 `ForeignWorkspace`
按 `RemoteRef` 持久化完整组合、呈现与恢复关系；本机投影仅登记本机拥有的 pane。重连或同机另一 GUI
从该记录重新挂载原会话，不以同值 `u64` 混淆远端 pane。另一台机器不拥有这些本机进程，不能伪造挂载。
同步使用带身份的语义变更重放；旧快照不覆盖较新的拆分或呈现，显式关闭与跨标签页移动独立表达。
显式关闭混合标签页时，本机 daemon 终止已无所有者的客户端本机会话；重排或跨标签页接管不终止会话。
关闭单个本机 pane 原子剪除本机投影、完整组合及呈现记录；保留其他所有者的同号 pane、存活视图和输入连接。
迟到的旧版本更新不能复活已关闭标签页或 leaf，也不能从新标签页夺回已转移的 pane；GUI 不将断线、缺少视图或仍有记录的已退出会话误判为关闭。
权威组合中的 pane 尚无 view 时，通过 attach-only 待恢复窗格保留原 ID 和所有者；附着失败显示失败与重试，不能回退为新建 shell。晚到的恢复附着只解除自身连接，不能终止原会话。

### 标签页内的停放牌堆

`TabPresentation.parked` 保存每个停放窗格的原分屏 sibling、轴、比例与侧，不保存整页快照。
停放 tiled 或 PiP 只改变可见投影，完整 logical tree 与 daemon 会话继续保留；PiP 停放时沿用原
分屏关系，恢复后只呈现 tiled。每次 Restore 只恢复选择的窗格，不取消其他停放项，也不回滚期间的
分屏与重排；原 sibling 失效时放到当前可见窗格旁，全部停放时第一张恢复卡片填满中央内容区。

`ParkPane` 与 `RestoreParkedPane` 可由菜单、随处搜索和自定义快捷键到达。全部停放时仍保留标签页、
卡片、恢复命令与新建窗格入口，不自动创建 shell。卡片与预览不挂载 `TerminalView`，也不接受终端输入。
还在树中的已退出窗格显示「已退出」并允许恢复查看；恢复本身不启动进程。明确关闭后删除其呈现记录，
过期恢复请求不能重建窗格或关闭其他会话。CLI 仍通过原 pane ID 发现、输入和捕获停放会话。

普通牌堆使用经闸门批准的原生完整变换：静止紧凑堆叠、hover 暖色扇形、仅目标卡片抬升／拉直／放大，
选择后翻面至 `120×112` 设计单位预览，显示真实标题、路径、运行状态与 Restore。尺寸随 rem 缩放；
系统 Latin/CJK 字体转换到 SVG alpha mask，公开 `paint_svg` 与 `TransformationMatrix` 共同变换外壳、
文字和背面，绘制与逆矩阵命中共享内容 viewport。暖色由 `presets::Parking.affordance` 独立交互角色提供，不能复用 warning。
每张重叠 sprite 拥有独立 paint layer；整个牌堆、光环、阴影、预览与命中始终裁切在兄弟状态栏以上。

方向键、Home/End、Enter 和 Escape 支持不依赖 hover 的选择／预览／恢复；Escape 或外部点击关闭预览并
恢复此前有效焦点。另一客户端收到持久停放关系时不会收到 hover、预览、键盘选择或动画进度；切换标签页
清除当前牌堆瞬态。减少动态直接到达相同端点，普通动画支持从当前位置反向中断，静止时不请求持续帧。
卡片预览入口与 Restore 是按窗格身份调用的原生按钮动作，不声明为列表选择项；辅助技术可直接激活指定卡片，不通过重叠矩形猜测目标。
停放终端的输出继续解析，但 `declare_displayed_panes` 必须声明其不可见，保留原有 Wakeup 通知门控，
不能因每包后台输出重绘整个窗口。无总停放数量上限；拖放目标与溢出入口不替代这套普通牌堆。
隐藏树使用后序迭代投影，每个节点只访问一次；不递归扫描可见后代，元素构建不随隐藏树深度叠加大栈帧，停放数量增长不能令剩余可见终端崩溃。

拖动平铺窗格（含最后一个或已缩放窗格）的顶部手柄，或 PiP 的顶部／左上移动手柄，靠近当前终端
内容区底部时显示暖色大半圆及「停放／Park」胶囊。大半圆以 `360×170` 设计单位随 rem 缩放，受当前
内容 viewport 限制；窄内容区裁切外侧弧线，不连带缩小胶囊文字与命中区。侧栏展开、收起、拖动宽度及
窗口／字号变化后重新解析。目标、胶囊、光环及其
阴影均使用同一内容裁切，不能依靠状态栏的绘制顺序遮住越界。目标命中使用当前已绘制半椭圆，
不是外接矩形；空白角与状态栏不接受停放，进入有效半圆时胶囊和轮廓加强。目标显示期间的空角释放
按取消处理，不借机执行普通分屏重排，也不提交 PiP 的临时位置。

只有左键在有效目标内释放才调用与菜单相同的 `park_pane`，不复制共享树、持久化或会话操作。
PiP 移动到目标时尚未提交的位置意图直接丢弃；恢复仍遵循原平铺关系。离开目标撤掉停放反馈，
普通重排／PiP 移动继续按各自有效落点解释；无效释放不改变呈现模式或会话身份。Escape、丢失按键、
离开客户区、窗口失活或切换标签页均清除本地手势与 capture，不把「拖动消失」推断为有效 drop。
取消不请求新焦点；有效停放由原停放命令选择可见终端或牌堆焦点。键盘／菜单入口不依赖此手势。

拖放消费者回归位于既有应用窗口 seam，覆盖最后一窗格、非焦点目标、PiP 两类手柄、Escape／无效释放、
离开目标、半圆空角及窄窗／放大字号／侧栏 resize 的边界。最终真实 Shell 拖放 smoke 与平台证据仍须
由集成验收执行，不能把未运行的回归当作原生通过。
「更多…」按实际内容 viewport 与当前 rem 预算展开卡片：保留原卡片尺寸、扇形间距与可用命中区，
另给入口留出独立空间；高度不足以完整呈现抬升卡片时也进入列表。它不是总停放数上限。
列表使用 `uniform_list`、`UniformListScrollHandle` 和原生 `Scrollbar`，只构建当前可见行；
当前行与恢复请求均绑定 App 注入的客户端 `EntityId` token，不按列表下标或 daemon pane ID 解释。
标题、路径、连接／运行状态来自同一张真实卡片，长内容有完整提示。

列表位于内容区内，保留足够行高和内部滚动，不借用窗口级浮层侵入状态栏。方向键、Home/End、
Page Up/Page Down 导航并滚动到当前项；Enter 先打开原生翻面预览，再恢复。Escape 逐层关闭预览、
列表，并返回有效触发点；入口随窗口变宽消失时返回牌堆。跨客户端重排保持 token，删除当前项
清除旧预览与旧命中；切换标签页同时清除本地列表状态。
列表与 focus trap 共用稳定元素身份，避免 focus trap 覆盖容器标识后无法定位真实布局边界。

新增应用 seam 回归包含二十个停放窗格、牌堆外键盘恢复、原相邻分屏被另一客户端替换、
窄窗／大字号／两侧栏切换以及当前预览被远端删除；本切片未运行检查。真实二十个 Shell 的
Windows 溢出列表、长/CJK 标题及缩放验收由集成阶段执行；没有实窗证据时不记为通过。


消费者回归覆盖高层应用 Action、焦点、跨客户端重建与旧快照重放，以及真实 daemon/CLI 的 shell 环境
连续性、重连、持久化、选择性恢复和退出／关闭语义。Windows 最终原生工作台验收仍需单独执行；
macOS 实窗已获明确豁免，不写成通过。



## 品牌图形维护

lyrel 使用已批准的 C「弦弧」图形：开放双臂、独立短弦和连续底弧。唯一几何来源是
`docs/brand/lyrel-icon.svg`（彩色应用图标）与 `docs/brand/lyrel-mark.svg`（单色标识）。
彩色图标底色为 `#29253F`、主弧形为 `#C5B6FF`、短弦为 `#8C7BD7`；单色前景由使用表面提供。
这些色值仅用于品牌资源，不进入界面主题、语义状态色或无关工具图标。

在仓库根目录运行 `uv run scripts/generate-brand-assets.py` 重新生成正式资源。脚本通过
内嵌依赖声明要求 Python 3.13+，固定 `resvg-py==0.5.0` 与 `pillow==13.0.0`，使用 resvg 栅格化定稿，
直接生成各尺寸图像，再写入 PNG、ICO 和 ICNS 容器；不从历史位图反向描摹或手绘新轮廓。
首次运行需能取得 Python 与这些依赖；它不是构建、测试或格式化步骤。

| 正式资源 | 来源与生产消费方 |
| --- | --- |
| `assets/app-icon.svg` | 彩色定稿；README、Windows 托盘 |
| `assets/app-icon.png` | 彩色定稿，1024×1024；macOS 裸二进制 Dock |
| `assets/logo.svg`、`assets/logo.png` | 彩色定稿；保留既有品牌资源入口，PNG 为 1024×1024 |
| `assets/logo@256.png` | 彩色定稿，256×256；非 macOS 窗口内品牌标记 |
| `assets/tray.svg` | 单色定稿；macOS 托盘 template image，系统根据菜单栏背景提供前景色 |
| `assets/favicon.ico` | 彩色定稿，16/24/32/48/64/128/256 像素帧；`build.rs` 嵌入 Windows EXE，Inno Setup 安装器使用同一资源 |
| `assets/lyrel.icns` | 彩色定稿，32/64/128/256/512/1024 像素图像；macOS bundle 脚本输入 |
| `docs/logo/logo.svg`、`docs/favicon.ico` | 彩色定稿；`docs/docs.json` 的深浅背景共用图标与 favicon，文档页面亦使用同一 logo |
| `assets/social-preview.png` | 定稿图形居中置于定稿底色，2560×1280；既有社交分享素材，不包含历史产品名或性能宣传 |

资源生产者与消费方使用统一的 lyrel 文件名；`assets/lyrel.icns` 复制到 bundle 内的
`Contents/Resources/lyrel.icns`，与 `CFBundleIconFile=lyrel` 保持一致。
PNG、ICO 和 ICNS 是已有版本管理约定中的正式资源，随本次图形变更提交；不要提交
中间尺寸文件、预览缓存、依赖目录或 Python 缓存。不要手改派生资源，应修改定稿后统一生成。

品牌评审只保留已批准的彩色与单色 SVG 定稿，不再维护比较稿或原型页面。项目许可证、
第三方图标许可和原始作者署名不因品牌替换而改变；图形定稿不代表已完成商标可用性检查。

### 图形验收边界

资源生成成功不等于生产视觉验收。集成后使用隔离配置启动真实工作台，观察窗口内标记、
应用身份、任务栏或 Dock 与托盘的小尺寸图形，并在深浅背景和 macOS 单色模板前景下检查
轮廓、透明边缘及已有托盘注意状态。保留平台、显示缩放和截图或可复核观察结果，不能用
SVG 浏览器预览、源码比较或编译通过替代真实 GUI 证据。

Windows 需重新构建后检查嵌入资源与任务栏，并在获授权的受控环境检查安装器图标；
macOS 需检查 `.app` 中的 ICNS、Finder/Dock 和菜单栏模板。Linux 不再提供 GUI、
AppImage 或桌面托盘；保留 headless 与远程开发。跨平台包内容、签名、公证、实际安装和系统注册分别记录，
不得把某一平台的本机观察推广为全部平台已通过，也不得为验收修改用户真实系统注册。

每次图形或打包变更分别记录资源生成、真实窗口、包内容及系统集成的验证结果。
没有对应平台和场景的运行证据时保持未验收；历史记录中的生成成功不能替代当前版本验收。
