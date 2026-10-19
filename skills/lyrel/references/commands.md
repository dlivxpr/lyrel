<a id="lyrel-command-reference"></a>
# `lyrel` 命令参考

本文列出所有子命令、各自的标志，以及使用 `--json` 时输出的 JSON。按需阅读相应章节；目录与顶层命令语法一致。

- [全局标志](#global-flags)
- [环境变量](#environment)
- [退出码](#exit-codes)
- [顶层子命令](#top-level-verbs)
- [`ws` — 工作区](#ws--workspaces)
- [`tab` — 标签页](#tab--tabs)
- [`pane` — 窗格](#pane--panes)
- [`machine` — 远程机器](#machine--remotes)
- [`server` — 守护进程](#server--the-daemon)
- [尚未实现](#not-implemented-yet)

<a id="global-flags"></a>
## 全局标志

可放在命令行的任何位置，子命令之前或之后均可。

| 标志 | 作用 |
|---|---|
| `-m, --machine <MACHINE>` | 通过本地服务器的现有连接，将命令路由到已连接的机器。可匹配完整连接键（`me@devbox:22`）或单独的主机名（`devbox`）。仅支持 SSH 连接；如果连接已断开，或涉及跳板机/代理链，则拒绝操作并说明原因，不会重新发起连接。 |
| `--json` | 在 stdout 上输出一个 JSON 对象，代替供人阅读的表格。 |
| `-q, --quiet` | 成功时不输出任何内容。错误仍输出到 stderr。 |

<a id="environment"></a>
## 环境变量

这些变量在每个 lyrel 窗格内设置，并由你从窗格中启动的所有程序继承。

| 变量 | 含义 |
|---|---|
| `LYREL_PANE` | 此窗格的 ID，例如 `71` 或 `%71`（两种形式均可接受）。它是 `split`、`send`、`exec`、`capture`、`procs`、`wait`、`pane close` 的默认目标。 |
| `LYREL_WS` | 此窗格的工作区 ID。它是 `run --keep`、`tab new`、`tab ls`、`ws tree` 的默认值。 |
| `LYREL_CONFIG_DIR` | 服务器的配置目录。CLI 通过它找到对应服务器的套接字，无需传入套接字路径。配置目录的优先级为 `--config-dir` > `LYREL_CONFIG_DIR` > Windows 便携模式的 `data` 目录 > 平台默认目录；Unix 默认为 `~/.config/lyrel`，Windows 默认为 `%APPDATA%\lyrel`，开发环境使用 `.lyrel-dev`。 |

在 lyrel shell 之外，需要目标地址的子命令会失败并报告：
`not inside a lyrel shell — pass an explicit %pane/@tab/workspace`。

<a id="exit-codes"></a>
## 退出码

| 退出码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 命令失败；原因以一行文本输出到 stderr，前缀为 `lyrel:` |
| 2 | 用法错误（clap）——未知子命令、缺少参数、类型错误 |
| 124 | `lyrel wait` 或 `lyrel exec` 放弃等待——遵循 `timeout(1)` 的惯例，以便区分“尚未完成”和“发生故障” |
| 141 | 仅限 Unix：读取方关闭了连接（`| head -1`），SIGPIPE 终止了进程，与终止 `cat` 的方式完全相同。这不算失败。Windows 没有可模拟的对应信号，同样情况下报告 0。 |
| *其他* | 仅来自 `lyrel run` 和 `lyrel exec`，它们会透传所运行命令的退出码 |

如果 `run` 或 `exec` 无法获知命令的退出码，会向 stderr 输出一条提示，以 1 退出，并在 JSON 中设置 `"exit_code_known": false`——据此可以区分真正的退出码 1 与替代值。

<a id="top-level-verbs"></a>
## 顶层子命令

<a id="lyrel-path"></a>
### `lyrel [PATH]`

不指定子命令即表示操作 GUI。若已有运行中的窗口，会请求它置于前台并在 `PATH` 处打开一个标签页；若没有已注册的窗口，则启动应用。不提供 `PATH` 时，只激活应用。不接受 `-m`——此命令控制的是*本机*的 GUI。JSON：`{"path","delivered","launched"}`。

如果此位置的单词不是路径，则会将其视为拼错的子命令并拒绝，而不是悄悄打开窗口。

<a id="lyrel-ls"></a>
### `lyrel ls`

等同于 `ws ls`。表格：`WORKSPACE NAME TABS PANES ATTACHED`。
JSON：`{"workspaces":[{"id","name","tabs","panes","attached"}]}`。

ATTACHED 表示持有工作区的主机——可能是 GUI 窗口，也可能是其他客户端——无人持有时为 `-`。这里只显示主机名；用于证明持有权的令牌绝不会离开拥有它的连接。

<a id="lyrel-run---keep---cwd-dir---ws-workspace----cmd"></a>
### `lyrel run [--keep] [--cwd DIR] [--ws WORKSPACE] -- CMD...`

创建一个运行 `CMD` 的窗格，将其输出流式传送到 stdout，等待命令结束，并以该命令的退出码退出。命令必须放在 `--` 之后；`--` 之后的所有内容都属于子进程，因此 `lyrel run -- cargo test --keep` 会把 `--keep` 传给 cargo。

- `--keep` 会在命令结束后将窗格保留为一个新标签页。此操作需要工作区，因此必须提供 `--ws` 或 `$LYREL_WS`；缺少工作区会报错，不会静默使用替代值。
- `--cwd` 设置工作目录。`--ws` 还会设置窗格的 `LYREL_WS`。
- 中断 `run` 可能留下一个孤立窗格——可通过 `pane ls --all` 查看。

JSON：`{"pane","exit","exit_code_known","kept"}`，在流式输出**之后**打印。合并后的输出流不是有效的 JSON；请读取最后一行。

<a id="lyrel-exec-pane---timeout-secs---raw----cmd"></a>
### `lyrel exec [%PANE] [--timeout SECS] [--raw] -- CMD...`

在现有窗格的 shell 中运行 `CMD`，并阻塞直到提示符重新出现：`--` 后的各个词会用空格连接，在提示符处作为一行输入，然后按 Enter，因此由该 shell 解释它们——请为 `&&`、`|` 和 `>` 加上引号，避免被你当前的 shell 解释。打印命令的输出，并以其退出码退出。目标窗格（默认为 `$LYREL_PANE`）会保留其工作目录、环境和历史记录，这正是它与 `run` 的区别：`run` 为一组 argv 创建新窗格，`exec` 则使用你已有的 shell。

shell 集成的 OSC 133 标记界定了命令范围：输出是窗格在命令开始（`C`）与结束（`D`，同时携带退出码）之间打印的内容。以下情况会拒绝执行，且不会输入任何内容：

- 窗格的 shell 尚未发送过提示符标记——没有集成，就永远不会有信号表明命令已结束；可运行 `lyrel doctor`，或改用 `send` + `wait --until free`
- 窗格当前不在提示符处——已有程序在其中运行（你自己的窗格总是如此：它正在运行 `lyrel exec`）
- 输入行包含控制字符（换行符、制表符、ESC），行编辑器会执行这些字符的动作，而不是将其作为文本输入——请用 `;` / `&&` 连接命令

| 标志 | 作用 |
|---|---|
| `--timeout SECS` | 在 SECS 秒后放弃等待，以 124 退出。命令仍继续运行；可用 `send %N --key C-c` 停止它。不设置超时则一直等待命令结束 |
| `--raw` | 按窗格接收到的原样输出，保留转义序列。默认会通过终端将其回放为文本，与 `capture --plain` 的读取方式相同——这也会擦除 zsh 在结束标记之前绘制的 PROMPT_SP |

如果 shell 始终未开始执行该行——例如未闭合的引号导致它等待续行提示符——就永远不会结束；超时消息会提示这可能是原因。如果 shell 直接拒绝该行（语法错误），则以 1 退出，设置 `"exit_code_known": false`，并将 shell 的报错作为输出。输出超过 8 MiB 时，会从开头丢弃超出部分，并向 stderr 输出提示。

JSON：`{"pane","command","exit","exit_code_known","timed_out","pane_exited","output","bytes","elapsed_ms"}`
——`output` 是打印形式的输出，`bytes` 是原始长度。

<a id="lyrel-new-path---open"></a>
### `lyrel new [PATH] [--open]`

创建工作区及其第一个标签页和 shell；如果给定 `PATH`，则以其为工作目录。打印工作区 ID。JSON：`{"id","pane","opened"}`。

如果本机正在运行 GUI，`--open` 还会为该工作区打开一个窗口——当你创建工作区是为了让别人查看时，请明确指定此选项。即使不指定，工作区仍会列在 GUI 的切换器中，只是在那里等待被打开。

<a id="lyrel-split-pane---v--h---ratio-r"></a>
### `lyrel split [%PANE] (--v|--h) [--ratio R]`

`pane split` 的别名。拆分 `%PANE`（默认为 `$LYREL_PANE`），并在相同工作目录中启动 shell。必须且只能指定一个方向——`--v`/`--vertical` 将新窗格放在下方，`--h`/`--horizontal` 将其放在右侧。`--ratio`（默认为 0.5）表示*原有*窗格保留的比例，会被限制到 0.05–0.95——`--ratio 70` 会静默变为 0.95，而不是报错。打印 `%NN`。JSON：`{"pane"}`。

<a id="lyrel-send-pane-text----stdin----from-file-path---paste---enter---key-key"></a>
### `lyrel send [%PANE] [TEXT | --stdin | --from-file PATH] [--paste] [--enter] [--key KEY]…`

以按键形式将 `TEXT` 输入窗格；`--enter` 是 `--key enter` 的简写——它会在文本后附加 CR；没有文本时则单独按 Enter，因此 `lyrel send %42 --enter` 会执行窗格 42 中已经输入的内容。只有一个参数时，该参数作为文本，窗格取自 `$LYREL_PANE`——但单独的 `%42`（或不带前缀的 `42`，即 `pane ls --json` 输出的形式）会被拒绝并报告缺少文本，而不是被输入；除非同时有 `--key` 指定要执行的动作。只有带 `%` 标记的写法才会把 `--enter` 视为满足这一条件的按键：`lyrel send 83 --enter` 会被拒绝，因为它既可能表示向自己的窗格输入 `83`，也可能表示在窗格 83 中按 Enter；错误消息会给出明确表达这两种意图的方法（`send %83 --enter`、`send %PANE 83 --enter`）。如果 `%` 后紧跟数字，但整体仍无法解析（`%3x`），则属于地址错误，绝不会作为文本输入自己的窗格——而仅以 `%` 开头的文本（`%s/foo/bar/`、`%!sort`）会按原样输入；不带标记且不是纯数字的内容（`3x`、`+5`）也是如此。若要输入形似地址的字符串，还需明确指定窗格：`lyrel send %42 %3x`。
JSON：`{"pane","sent","source","bytes","paste","bracketed","bracketed_paste_mode","enter","keys"}`
——`source` 为 `argument`、`stdin`、`file` 或 `none`。

`--stdin` / `--from-file PATH` 从相应来源读取文本，而不是使用参数——适合传递机密信息，因为作为参数的机密会出现在 `ps` 中，供所有本地用户查看，也会进入 shell 历史记录。字节会完全按读取时的原样发送到窗格（不解释转义、不转换换行、不要求 UTF-8），且 JSON 中的 `sent` 为 `null`，因此也不会被回显到日志。使用其中任一选项时，唯一的位置参数是窗格，且必须为有效地址。请使用 `printf %s`，而不是 `echo`，除非你确实需要末尾的换行符——在 pty 上，该换行符就是 Enter。对端是否回显机密取决于读取它的程序，而非 lyrel：`docker login --password-stdin`、`ssh-add` 和 `gpg` 都会关闭回显。

`--paste` 按 GUI 粘贴的方式发送文本。如果窗格已启用括号粘贴模式（模式 2004——处于提示符的 shell 和大多数 TUI 都会启用），文本会被包裹在 `ESC[200~ … ESC[201~` 中，CRLF 会转换为 LF，且每个 ESC 字节都会被移除，以防文本提前结束粘贴；这样，多行文本会作为一个整体落入窗格，而不是逐行执行。如果窗格未启用该模式，则不添加包裹标记，每个换行都作为 Enter 发送——这与向此类窗格粘贴时一贯的行为相同——同时会向 stderr 输出提示。`bracketed` 表示实际采用哪种发送方式；`bracketed_paste_mode` 表示窗格报告的模式状态（服务器过旧、无法报告时为 `null`）。在 `--paste` 之后使用 `--enter` 会提交粘贴的文本块。

`--key` 用于按键，而不是输入字符——例如权限提示所需的方向键、关闭 TUI 的 `escape`、停止构建的 `C-c`。可重复指定，按顺序发送，并可与 `TEXT` 组合使用（先发送文本）。

| 类别 | 按键 |
|---|---|
| 命名按键 | `enter` `escape` `tab` `backtab` `space` `backspace` `delete` `up` `down` `right` `left` `home` `end` `pageup` `pagedown` |
| 组合键 | `C-<char>`（Ctrl：`C-c`、`C-d`、`C-z`，以及 `C-@ C-[ C-\ C-] C-^ C-_ C-?`），`M-<char>`（Alt = 添加 ESC 前缀） |
| 别名 | `return` `cr` `esc` `del` `bs` `shift-tab` `pgup` `pgdn` `pgdown` |

不区分大小写，但有一个例外：Alt 是添加 ESC 前缀，因此字符会严格按原样发送，`M-X` 不等于 `M-x`（Ctrl 不受影响：`C-c` 和 `C-C` 是同一个字节）。未知名称会触发用法错误（退出码 2），且发生在写入任何内容之前，因此错误按键绝不会把半截序列发送到正在运行的窗格。每次按键都作为独立事件发送，与前一次写入间隔 200 ms，以防原始模式的 TUI 将序列当成粘贴；第一次写入没有延迟，因此中断会立即生效。

<a id="lyrel-capture-pane---plain---scrollback---tail-n"></a>
### `lyrel capture [%PANE] [--plain] [--scrollback] [--tail N]`

获取窗格的回放。有三个相互独立的选择：**读取多少内容**——默认读取最新的回滚分段，使用 `--scrollback` 则读取整个环形缓冲区（调整尺寸时会对环形缓冲区分段，因此对于从未调整过尺寸的窗格，两者相同）——**采用什么形式**，以及**保留多少行**。

默认范围的边界是窗格上一次*调整尺寸*的位置，而调整尺寸是窗口事件，不是窗格输出中的事件：调整尺寸会封存包含此前全部输出的分段；在 Unix 上，shell 会响应 SIGWINCH，在新分段中重绘提示符——因此刚调整尺寸后，最新分段可能只有这次重绘，没有其他内容，而命令输出仍位于前一个分段中。字节流中没有任何信息能区分重绘与真正的输出，因此 `capture` 无法替你判断。读取窗口尺寸可能变化过的窗格时——GUI 中的窗格都属于这种情况——请使用 `--scrollback`；如果只需要末尾内容，再用 `--tail N` 截取。

不使用 `--plain` 时，会得到存储的字节，完整保留 ANSI 转义序列，并按 UTF-8 解码（无效字节替换为 U+FFFD）。这是忠实保留的形式：它就是窗格实际输出的内容。

使用 `--plain` 时，这些字节会通过终端网格回放——使用与 GUI 渲染窗格相同的解析器和版本——返回由此产生的文本。它与自行剥除转义序列的区别在于：

- shell 按窗格宽度自动折行的一行会以**一行**返回，不会被人为添加的换行符拆开
- `\r` 会**覆盖**而不是换行，因此进度条呈现最终值，带语法高亮的 shell 也不会回显成 `eecho …echo`
- 光标定位（`ESC[5;80H`）会把文本放在**应用实际放置的位置**
- 宽字符在每对单元格中保留一个字符；组合标记保持原位
- 每个分段都按**自身的**宽度渲染，这也是尺寸信息随分段保存的原因；网格在输出上下留下的空行会被移除

只要是为了让人阅读输出，就应使用它。不过，它仍然只是屏幕：滚出顶部的内容已经消失，退出码也从未显示在屏幕上——如果你需要的是结果而不是画面，请重定向到文件。

`--tail N` 只保留结果的最后 N 行，丢弃其余内容——无需通过管道调用 Windows 上没有的 `tail(1)`，就能查看“上一条命令是如何结束的”。它在最后执行裁剪，即先由 `--plain` 确定什么算一行，因此 shell 自动折行的一行只计一次：`--plain --tail 1` 返回完整的最后一行，而不只是它在屏幕上的最后一排。`N` 必须至少为 1——不保留任何行会让结果看起来像空白窗格。服务器仍会回放整个环形缓冲区，因此省去的是管道处理，而不是网络传输。

无论哪种形式，它都是快照，不是流：先收集服务器发送的回放，等待约 300 ms 让数据稳定，然后返回。再次调用即可获取更新的快照。
JSON：`{"pane","text","bytes"}`，其中 `text` 是请求的形式，包含 `--tail` 的处理结果；`bytes` 则是回放携带的数据量，在 `--plain` 渲染或 `--tail` 裁剪之前计数。`bytes` 可用于区分空 `text` 的不同情况：`0` 表示窗格从未打印任何内容；字节数非零但没有文本，则表示这些字节未产生可见内容——例如窗格被清空。这第二种情况还会向 stderr 打印一行提示，因此即使脚本只读取 stdout，这一情况也仍会被提示出来。

<a id="lyrel-procs-pane"></a>
### `lyrel procs [%PANE]`

显示窗格内的进程树，按深度缩进，前台进程带有 `*`——随后以第二张表列出这些进程正在监听的端口。两者均为空时打印 `nothing running in this pane`。

JSON：`{"procs":[{"pid","name","depth","foreground"}],"ports":[{"port","pid","name","addr"}],"context":{…}}`，
其中 `addr` 是套接字绑定的地址（`*`、`0.0.0.0`、`127.0.0.1`、`[::1]` 或某个特定接口）。`context` 是窗格报告的自身信息：`local_pty`、`remote`（其指向的主机）、`at_prompt`（来自 shell 集成的标记；首个标记到达之前不存在）、`remote_prompt_seen`，以及 `bracketed_paste`（窗格是否开启模式 2004——即 `send --paste` 读取的状态）。

深度为 0 的 shell 下没有任何进程，意味着前台命令已退出——但通常不必手动检查，因为这正是 `lyrel wait --until free` 阻塞等待的条件。

<a id="lyrel-agents"></a>
### `lyrel agents`

列出所有正在运行已识别编程智能体的窗格。表格：`PANE AGENT STATUS MESSAGE`，状态为 `idle` / `working` / `waiting` / `done` 之一。
JSON：`{"agents":[...]}`；如果某个运行中的智能体缺少状态钩子或钩子已过期，还会附带 `"diagnostics"`——这正是智能体可能永远停留在某个状态的原因。每条诊断为：
`{"kind":"agent_status_hooks_unavailable","agent","hooks_state","action"}`。

<a id="lyrel-wait-pane---until-state---changed---timeout-secs---interval-ms"></a>
### `lyrel wait [%PANE] [--until STATE,…] [--changed] [--timeout SECS] [--interval MS]`

阻塞直到窗格进入指定状态之一。这是编排的基础操作：`lyrel wait %3 && lyrel capture %3 --plain`。

| 标志 | 默认值 | 作用 |
|---|---|---|
| `--until` | `waiting,done,exit` | 以逗号分隔；参见下方状态 |
| `--changed` | 关闭 | 仅在窗格进入等待开始*之后*才转入的状态时唤醒 |
| `--timeout` | 无 | N 秒后放弃等待，以 124 退出 |
| `--interval` | 500 | 轮询间隔，单位为 ms（50–3,600,000） |

| 状态 | 含义 |
|---|---|
| `idle` `working` `waiting` `done` | 智能体通过自身钩子报告的状态 |
| `no-agent` | 此处没有程序报告状态——可能是普通 shell，也可能未安装钩子 |
| `free` | 前台命令已退出；窗格回到只有 shell 的状态 |
| `exit` | 窗格已消失。无论是否指定，都会结束所有等待 |

JSON：`{"pane","status","matched","stale","activity","message","session_id"}`。
`stale: true` 表示等待开始时窗格就已经处于该状态，因此返回的结果可能属于上一轮——`--changed` 正是用来排除这种情况的。

退出码 0 = 已达到请求的状态；124 = 超时；1 = 窗格未达到请求状态便已退出（仍会输出 JSON，其中 `"matched": false`）。

以下细节决定循环能否正常工作：

- **`idle` 不表示“命令已完成”。** 它是*智能体*对自身状态的描述。运行构建的窗格没有智能体，会报告 `no-agent`。等待命令时请使用 `free`。
- **检查 `free` 每轮轮询需要额外发起一次请求**，因此只有明确指定时才检查，而且仅在你请求的智能体状态均未先匹配时才检查——组合使用 `waiting,done,free` 不会漏掉 `waiting`。
- **对于 `free`，`--changed` 的含义不同**：shell 从空闲 → 忙碌 → 空闲，最终回到起始状态，因此没有可用于比较的新状态。此时它表示“在我观察期间有程序运行过”——这正是你在 `send` 之后需要的行为。如果命令快到在一个 `--interval` 内就已完成，就永远观察不到其运行状态，最终会超时；对于这种命令，请使用 `--interval 100`，或去掉 `--changed` 并读取哨兵文件。
- **`free` 读取的是进程树**，因此如果窗格的根进程就是命令本身（`lyrel run` 创建的窗格），它在运行时也会看起来处于空闲状态；而后台作业则会在前台命令退出后仍让窗格保持忙碌。

<a id="lyrel-events"></a>
### `lyrel events`

持续输出服务器事件，直到被中断，每行一个——包括窗格退出、智能体状态变化、工作区控制权被抢占、布局增量变化。`--json` 会将输出变为 NDJSON。它会一直阻塞；请设置超时运行，或放到后台。

<a id="lyrel-status"></a>
### `lyrel status`

等同于 `server status`：显示 PID、运行时长、窗格数、通信方言版本、构建信息、套接字路径。JSON 就是 `ServerStatus` 对象本身（`pid`、`uptime_secs`、`panes`、`control_version`、`protocol_version`、`build`、`socket`）。

<a id="lyrel-doctor"></a>
### `lyrel doctor`

检查安装情况：三个环境变量、服务器是否响应、其控制/协议版本是否与当前二进制匹配、PID/运行时长/窗格数、机器连接数量，以及各智能体状态钩子的情况。如果当前不在 lyrel shell 中，还会添加提示。
JSON：`{"context":{"config_dir","workspace","pane"},"server":{"reachable","dialect_ok","build","status","routes"},"hooks":{"installed","outdated","not_installed"}}`
——上下文字段是布尔值，而不是实际的配置值；每个 `hooks` 字段都是智能体 slug 的列表。

钩子行能解释为什么智能体一直不变：没有钩子就不会报告状态，因此 `lyrel agents` 会显示它停滞不变，`lyrel wait` 则只会不断超时。钩子安装在本地，因此使用 `-m` 时，该行显示 `unknown`，而不是对其无法查看的安装情况妄下结论。

<a id="ws--workspaces"></a>
## `ws` — 工作区

工作区是服务器持续维护的一棵命名树，由标签页和窗格组成。可按名称、完整 ID 或唯一 ID 前缀指定工作区（通常使用 `lyrel ls` 打印的 8 字符前缀）。名称或前缀存在歧义时会报错，并列出候选项。

| 命令 | 作用 | JSON |
|---|---|---|
| `ws ls` | 列出所有工作区 | `{"workspaces":[...]}` |
| `ws tree [WORKSPACE]` | 以树形显示一个工作区：标签页、拆分方向及比例、窗格及其工作目录 | 完整工作区对象：`{"id","name","last_active","tabs":[{"id","name","sidebar_group","root",...}]}`，其中 `root` 是嵌套的拆分树 |
| `ws new [NAME]` | 创建空工作区（没有标签页，也没有窗格） | `{"id","name"}` |
| `ws rename WORKSPACE NAME` | 命名或重命名 | `{"id","name"}` |
| `ws rm WORKSPACE` | 删除工作区并挂断其窗格 | `{"removed"}` |
| `ws attach WORKSPACE` | 成为其控制客户端 | `{"attached","took_over_from"}` |
| `ws detach WORKSPACE` | 释放控制权，不中断任何运行内容 | `{"detached"}` |

`ws rm` 会挂断工作区持有的窗格，因此仅删除临时工作区就足以完成清理；如果它报告某些窗格无法挂断，这些窗格会继续运行，并作为孤立窗格出现在 `pane ls --all` 中。另一种泄漏来自被中断的 `lyrel run`：该窗格会继续运行，却没有任何对象引用它。`pane ls --all` 可以找到这些窗格。

需要可直接使用的工作区时，优先使用 `lyrel new <path>`，而不是 `ws new`：`ws new` 只会留下一个需要你继续填充的空工作区，而 `lyrel new --json <path>` 会返回 `{"id","pane"}`——一次获得两个地址。

`ws tree --json` 中的 `root` 节点采用外部标签形式，因此叶节点为 `{"Leaf":{"pane":31}}`，拆分节点为 `{"Split":{"axis","ratio","a","b"}}`，其中 `a`/`b` 以相同方式嵌套。`d["tabs"][0]["root"]["pane"]` 无法使用。

<a id="tab--tabs"></a>
## `tab` — 标签页

`@N` 按树顺序为**整台机器**上的标签页编号，从 `@1` 开始连续递增。任何工作区或标签页的创建或删除都会改变编号，因此应在使用前立即解析。也可使用完整的标签页 UUID：`@<uuid>`。

| 命令 | 作用 | JSON |
|---|---|---|
| `tab ls [WORKSPACE]` | 列出工作区的标签页 | `{"workspace","tabs":[{"ordinal","id","name","label","agent","group","panes":[..]}]}` |
| `tab new [WORKSPACE] [--cwd DIR]` | 添加带有新 shell 的标签页 | `{"tab","pane"}` |
| `tab close @TAB` | 关闭标签页及其中所有窗格 | `{"closed"}` |
| `tab rename @TAB NAME` | 命名或重命名 | `{"tab","name"}` |
| `tab move @TAB INDEX` | 在所属工作区内调整位置 | `{"tab","to"}` |

GROUP 是 GUI 侧边栏用于归类标签页的分组标题，显示其最后一个路径段（JSON 中的 `group` 是完整值）。此处只读：使用默认的仓库分组方式时，GUI 会根据标签页的工作目录重新计算它，因此从外部写入的任何内容都会在下一次渲染时被覆盖。

几乎没有标签页设置了 `name`：GUI 的标签栏读取 OSC 标题，而机器树无法看到这些标题。因此 NAME 列以及 JSON 中的 `label` 会依次选择可获得的最佳信息：如果有人设置过名称，就使用该名称；否则使用标签页中运行的智能体（如“omp”）；再否则使用工作目录的最后一个路径段；最后使用前台进程。JSON 中的 `name` 仍保留其实际值，因此脚本仍能区分真正的名称与替代显示值。

<a id="pane--panes"></a>
## `pane` — 窗格

| 命令 | 作用 | JSON |
|---|---|---|
| `pane ls [WORKSPACE]` | 列出窗格及其工作区、标签页、工作目录、存活标志 | `{"panes":[...]}` |
| `pane ls --all` | 列出服务器的完整窗格注册表，包括不被任何工作区持有的孤立窗格 | `{"panes":[...],"orphans":N}` |
| `pane split ...` | 与顶层 `split` 相同 | `{"pane"}` |
| `pane close [%PANE…]` | 关闭窗格；挂断其 shell | `{"closed":[...]}` |
| `pane close --orphans` | 关闭所有不被任何工作区持有的窗格 | `{"closed":[...]}` |

`--all` 用于显示泄漏的窗格。每个条目为
`{"pane","workspace","orphan","owner","title","cwd","live"}`：`owner` 是允许附加到该窗格的工作区 ID（如果没有工作区可以附加，则此字段不存在——例如尚未由 `--keep` 归入工作区的独立 `lyrel run` 窗格），`orphan: true` 表示没有工作区持有它。被中断的 `lyrel run` 会留下这种窗格。

`close` 一次接受多个 ID，即使某个失败也会继续尝试其余 ID，并以 1 退出，返回 `{"closed":[...],"failed":[...]}`，让你知道还剩哪些。`--orphans` 只关闭 `pane ls --all` 标记为孤立的窗格；没有需要处理的窗格时返回空列表，而不是报错。

**`--orphans` 是用户的清理工具，不是你可以自行使用的工具。** 它会关闭机器上的所有遗留窗格，而遗留窗格可能仍在运行某人的命令。请向用户指出这个选项，不要擅自运行。

`title` 是窗格当前的标题——通常是正在运行的命令，因此会显示为 `omp`、`nvim`、`cargo`——这使得 `pane ls --all --json` 成为快速查找“正在运行 X 的窗格”的方法，无需捕获任何内容。

<a id="machine--remotes"></a>
## `machine` — 远程机器

`machine ls` 列出本地机器以及服务器持有的所有连接：
`MACHINE KIND CONNECTED`。JSON：`{"machines":[{"key","kind","connected"}]}`。

`machine connect` / `machine disconnect` 尚未实现——连接由 GUI 的连接管理器管理。

<a id="server--the-daemon"></a>
## `server` — 守护进程

| 命令 | 作用 |
|---|---|
| `server status` | 等同于 `lyrel status` |
| `server logs` | 查看服务器日志末尾；打印日志路径，如果从未启用日志，也会明确提示（需在服务器启动前设置 `LYREL_LOG=info`） |
| `server start` | 在本机启动服务器 |
| `server stop` | 停止服务器——**机器上的所有窗格都会终止** |
| `server restart` | 先停止再启动——后果相同 |

不要擅自运行 `start`、`stop` 或 `restart`。它们会改变或销毁用户 GUI 当前附加的服务器状态。

<a id="not-implemented-yet"></a>
## 尚未实现

以下命令可被解析，但随后会以 1 退出并说明原因：

- `ws stop`——控制方言尚无停止工作区的请求
- `machine connect` / `machine disconnect`——请使用 GUI