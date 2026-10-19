---
name: lyrel
description: >-
  在 shell 中通过 `lyrel` 二进制程序操控 lyrel 终端工作台——列出工作区、标签页和窗格，拆分窗格，向窗格发送文本或按键，捕获窗格屏幕内容，在真实 PTY 中运行命令并透传其退出码，在已有的 shell 窗格中运行命令并获取其输出和退出码，阻塞等待窗格完成任务或需要输入，以及查看正在运行的编程智能体和窗格正在监听的端口。以下情形都应使用本技能：提及 lyrel、窗格、工作区、`%42`、`@7`、“另一个窗格”或“另一个智能体”时；希望将工作交给另一个智能体并收集结果时（“让 omp/pi 做 X”、“派个活”、“让另一个智能体处理这个”，或并行运行多个智能体并合并其产出）；需要启动不应阻塞 Bash 工具的长期运行或交互式程序时（开发服务器、REPL、ssh 会话、`tail -f`、TUI）；程序需要真实终端才能表现得与用户所见一致时；以及需要查看或报告本机某个其他终端中正在运行的内容时。检查成本很低：如果设置了 `$LYREL_PANE`，说明你已经在 lyrel 内部，这里的所有命令都无需额外设置即可使用。
---

<a id="driving-lyrel-from-the-command-line"></a>
# 从命令行操控 lyrel

`lyrel` 是 lyrel 服务器的轻量级非交互式客户端。每个操作都会返回并退出；`--json` 让输出变为机器可读格式。GUI 完全不必运行——窗格由服务器持有。

<a id="first-where-are-you"></a>
## 首先：你在哪里？

```bash
lyrel doctor
```

只需一张表，就能回答你在采取任何其他操作前需要了解的所有问题：能否连接到服务器、协议方言是否匹配、各智能体的状态钩子是否已安装，以及是否设置了 `LYREL_CONFIG_DIR` / `LYREL_WS` / `LYREL_PANE`——也就是你是否正在 lyrel 窗格*内部*运行。

身处窗格内部之所以重要，有两个原因：接受目标地址的操作（`split`、`send`、`exec`、`capture`、`procs`、`wait`、`pane close`）默认使用 `$LYREL_PANE`，而 `run --keep` 会将其窗格放入 `$LYREL_WS`。在 lyrel shell 外部，必须显式指定目标；否则会给出错误，而不会自行猜测。

如果你打算将任务委派给另一个智能体，就需要关注钩子所在行：没有这些钩子，智能体就不会报告状态，对它执行 `lyrel wait` 只会超时。

如果 `lyrel doctor` 表示无法连接服务器，请停止操作并告知用户——不要擅自运行 `lyrel server start`。启动用户未要求启动的服务器，会改变其 GUI 所连接的目标。

<a id="what-are-you-here-to-do"></a>
## 你要在这里做什么？

四种任务，四种方式：

1. **运行不应阻塞你或需要真实 TTY 的程序**——开发服务器、耗时较长的测试、TUI。[运行命令](#running-a-command-two-shapes)。
2. **持续与有状态的程序交互**——REPL、`ssh`、调试器。使用相同的基础操作：[发送](#non-blocking-a-pane-you-talk-to-over-time)、[读取](#reading-a-pane)，然后重复。
3. **查看本机正在做什么**——其他窗格、其他智能体、端口。[查看概况](#looking-around)，严格只读。
4. **将工作交给另一个编程智能体**——一个工作智能体，或并行分派给多个工作智能体。先阅读 `references/delegation.md`；简要说明见[下文](#handing-work-to-another-agent)。

对于启动后完成任务并退出、既不需要终端也不需要供人观看的操作，Bash 工具仍然是合适的选择。当进程需要在你的本轮交互结束后继续运行、需要真实 PTY，或应让用户在其 lyrel 窗口中看到时，窗格才有价值——最后一点往往正是使用它的全部意义。

<a id="addresses"></a>
## 地址

| 形式 | 含义 | 是否稳定？ |
|---|---|---|
| `%42` | 一个窗格 | 是——窗格在整个生命周期内保留其 ID |
| `@7` | 一个标签页，按树顺序在**整台机器**范围内编号 | **否**——工作区或标签页出现或消失时，编号都会变化 |
| `@<full tab UUID>` | 同一个标签页，通过 ID 指定 | 是 |
| `api` / `76698a44` / 完整 UUID | 一个工作区，通过名称、唯一 ID 前缀或 ID 指定 | 是 |

在使用 `@N` 前立即重新解析；绝不要跨越会创建或删除标签页的步骤缓存它。窗格 ID、标签页 ID 和工作区 ID 都可以安全保存——因此，如果你创建标签页后打算稍后再次访问它，应保留 `lyrel tab new --json` 返回的 ID，而不是再次计数得到 `@N`。

在任何接受地址的位置，前缀符号都是可选的：`%42` 和 `42` 表示同一个窗格，`@7` 和 `7` 表示同一个标签页。从 `--json` 输出复制的 ID 可以直接粘贴回来使用。

在 lyrel shell 内省略地址，意味着“此窗格”或“此工作区”。显式地址始终优先于环境变量。

<a id="running-a-command-two-shapes"></a>
<a id="running-a-command-three-shapes"></a>
## 运行命令：三种方式

<a id="blocking-with-a-real-exit-code"></a>
### 阻塞运行，获得真实退出码

```bash
lyrel run -- cargo test          # 将输出流式传到你的 stdout，并以 cargo 的退出码退出
lyrel run --cwd /path -- make
lyrel run --keep -- cargo build  # 完成后将窗格保留为一个新标签页
```

命令输出会实时流式传到你的 stdout，`lyrel` 则以命令自身的退出码退出。这是最接近 Bash 调用的方式——区别在于它使用 PTY，而且用户能看到运行过程。

有三点需要了解。`--keep` 需要工作区，因此只能在 lyrel shell 内使用，或配合 `--ws <workspace>` 使用。使用 `--json` 时，先输出流式内容，最后输出 JSON 对象——合并后的输出流*无法*作为 JSON 解析，因此应读取最后一行。另外，窗格宽度固定为 120 列，无法更改，因此假定终端更宽的输出会换行。

<a id="blocking-in-a-pane-you-already-have"></a>
### 在已有窗格中阻塞运行

```bash
lyrel exec "$PANE" --timeout 900 -- cargo test   # 打印命令输出，并以其退出码退出
```

`exec` 会在窗格的 shell 提示符处输入命令行——使用该 shell 的 cwd、环境和历史记录——等待提示符重新出现，打印命令的输出（以文本形式，类似 `capture --plain`；`--raw` 会保留转义序列），然后以命令自身的退出码退出。通过它，你可以在之前已设置好的窗格中运行下一条命令（例如已经执行过 `cd`、激活过 venv，或建立了远端带有 lyrel shell 集成的 `ssh` 会话），而不必反复发送、等待、捕获和猜测。

它的要求以及不满足要求时的反馈如下：窗格的 shell 必须具有 lyrel shell 集成（从未发送过提示符标记的窗格会被拒绝），而且窗格必须停留在提示符处（忙碌的窗格会被拒绝——先执行 `wait --until free`）。`--` 后的内容会组成一行 shell 命令，因此只要正确引用，就可以使用 `&&`、管道和重定向：`lyrel exec %3 -- 'make && ./run'`。无人看守时务必指定 `--timeout`：如果一行命令始终未被 shell 执行——例如引号未闭合，停在 `dquote>` 等待输入——就永远等不到结束。超时时，它以 124 退出，并让命令继续运行。

<a id="non-blocking-a-pane-you-talk-to-over-time"></a>
### 非阻塞运行：持续与窗格交互

这正是 lyrel 值得使用的地方。获取一个窗格，给它发送工作，稍后再回来查看。

```bash
PANE=$(lyrel split --v)                  # 也可以用 --h；拆分 $LYREL_PANE，打印 "%83"
lyrel send "$PANE" 'npm run dev' --enter
```

`split` 会将新窗格的地址打印到 stdout，你可以将其捕获到变量中。不指定方向会产生用法错误——`--v` 将新窗格放在下方，`--h` 将其放在右侧。

拆分 `$LYREL_PANE` 会改变用户可见的布局，而这通常正是目的所在：用户可以看到你启动的开发服务器。请告知用户你做了此操作，并在使用完后关闭窗格。

如果你*不在* lyrel 窗格内，就没有可拆分的目标，因此应先创建自己的工作空间。`lyrel new --json /path/to/repo` 会一次返回两个 ID——不要再通过 `ws tree` 翻找窗格：

```bash
read -r WS PANE < <(lyrel new --json /path/to/repo \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["id"], "%%%d" % d["pane"])')
```

`send` 会像键盘一样向窗格输入文本；`--enter` 会在末尾附加回车，或者在未提供文本时单独按下 Enter（`lyrel send "$PANE" --enter` 会执行那里已经输入的内容）。它不会等待，也不会告诉你发生了什么——读取是独立步骤，等待则使用 `lyrel wait`。

对于 Ctrl-C、Escape、方向键等按键，而非普通字符，请使用 `--key`：它接受 `enter escape tab backtab space backspace delete up down right left home end pageup pagedown`，以及表示 Ctrl 的 `C-<char>` 和表示 Alt 的 `M-<char>`。重复使用该选项可以组成按键序列；文本与按键可以组合使用，先输入文本。将 `^C` 作为文本输入不会起到中断作用——它会作为两个字符到达；`--key C-c` 才是真正的中断。

有两类内容不能直接作为命令行文本处理。**秘密信息**：`lyrel send %3 "$TOKEN"` 会让令牌出现在所有本地用户可见的 `ps` 输出和你的 shell 历史中——应改用管道：`printf %s "$TOKEN" | lyrel send %3 --stdin --enter`（或 `--from-file PATH`）；字节会按读取时的原样发送到窗格，绝不会在 `--json` 中回显。接收端是否回显由程序决定：`docker login --password-stdin`、`ssh-add` 和 `gpg` 会自行关闭回显。
**多行文本**：在 PTY 上，换行*就是* Enter，因此每一行到达时都会执行。`--paste` 会以粘贴到窗口的方式发送——如果窗格已启用括号粘贴模式，就会加上相应边界标记，让 shell 或智能体输入框将整块内容当作文本接收；`--json` 会报告 `"bracketed"`，让你知道实际采用了哪种方式。

**全新的窗格可能吞掉 Enter。** 如果 shell 仍在处理启动文件——提示符框架、`fastfetch` 或任何登录时绘制界面的程序——它可能接收你发送的文本，却丢掉随后到达的回车，使得命令只停留在提示符行上而未执行。不会有任何报告提示此事：`send` 成功了，而窗格看起来只是一个尚未开始工作的工作智能体。因此，在向刚创建的窗格发送第一条命令后，应读取屏幕，检查命令是否确实已离开提示符开始执行：

```bash
lyrel capture "$PANE" --plain | tail -3   # 命令是否仍停留在提示符上？
lyrel send "$PANE" --enter                # 如果是，就补上丢失的 Enter
```

这比事后诊断省事，而且只有向新窗格进行第一次 `send` 时才需要检查。

<a id="reading-a-pane"></a>
## 读取窗格

<a id="if-you-want-the-screen-use---plain"></a>
### 如果想看屏幕，使用 `--plain`

```bash
lyrel capture %83 --plain
```

`capture` 会返回守护进程存储的内容——窗格的字节，包括所有转义序列——而 `--plain` 会通过终端网格重放这些内容，转而打印得到的文本。它不只是简单删除转义序列：除了移除颜色和光标转义序列，shell 因窗格宽度而折行的内容也会重新合为一行，用 `\r` 反复覆盖的进度条会呈现最终值，TUI 屏幕内容也会出现在原本绘制的位置。凡是人需要阅读输出时，都应使用它。

无论采用哪种方式，返回内容都有两个细节：capture 返回的是*快照*，而不是流——再次调用才能获取更新的快照。另外，默认只打印最新的回滚缓冲区片段（环形缓冲区会在调整大小时分段）；`--scrollback` 则打印整个环形缓冲区，对于从未调整过大小的窗格，两者相同。

<a id="if-you-want-the-result-redirect-to-a-file"></a>
### 如果想要结果，重定向到文件

`--plain` 提供的是屏幕，而屏幕只是一个矩形：长构建日志中滚过顶部的内容已经不在了，退出码更是从未出现在屏幕上。当窗格中的 shell 停在提示符处时，上述 `lyrel exec` 可以同时返回两者。否则——窗格正忙，或其 shell 没有 lyrel 集成——应让 shell 将结果写到干净的文件中：

```bash
lyrel send "$PANE" 'cargo test > /tmp/t.log 2>&1; echo $? > /tmp/t.rc' --enter
# ……等待完成（见下文），然后：
cat /tmp/t.rc /tmp/t.log
```

完整输出、真实退出码，中间不经过终端呈现。

<a id="knowing-when-a-command-has-finished"></a>
### 判断命令何时完成

不要轮询屏幕，也不要自己编写循环——阻塞等待即可：

```bash
lyrel wait "$PANE" --until free --changed --timeout 900
```

`free` 表示前台命令已经退出，窗格回到了只有 shell 的状态。`--changed` 额外要求“在我观察期间确实运行过某个程序”，这正是紧接在 `send` 后面时需要的条件：没有它，尚未启动的命令会使窗格看起来已经完成。

从头到尾的完整流程：

```bash
lyrel send "$PANE" 'cargo test > /tmp/t.log 2>&1; echo $? > /tmp/t.rc' --enter
lyrel wait "$PANE" --until free --changed --timeout 900
cat /tmp/t.rc /tmp/t.log
lyrel pane close "$PANE"
```

退出码就是为此设计的：`0` 表示已达到你要求的某个状态，`124` 表示等待超时（遵循 `timeout(1)` 约定，因此可以区分“尚未完成”和“发生故障”），`1` 表示窗格先终止了。

`--changed` 有一个陷阱：如果命令在一次轮询间隔（默认 500ms）内就完成，就永远不会被*观察到*处于运行状态，因此会一直等待到超时。对于这么快的命令，可以使用 `--interval 100`，或者去掉 `--changed` 并读取 `.rc` 文件。发生这种情况时，超时消息会作出说明。

如果你想查看进程树本身——“里面正在运行什么”、“这个窗格正在通过哪个端口提供服务”——应使用 `lyrel procs %83`：按深度缩进，用 `*` 标记前台进程，然后列出这些进程正在监听的端口。不过，这不是检查编程智能体的方法：即使窗格中有忙碌的智能体，`procs` 也会报告 `nothing running in this pane`，因此将其理解为“工作智能体已终止”是错误的。应使用 `lyrel agents` 查询它们。

<a id="handing-work-to-another-agent"></a>
## 将工作交给另一个智能体

窗格可以承载另一个编程智能体，上述所有基础操作都适用——另外还有一项智能体专有能力：状态钩子会报告 `working` / `waiting` / `done`，因此 `lyrel wait` 可以针对*智能体*而非其进程树进行阻塞等待。

**委派有专门的操作指南——`references/delegation.md`。在启动工作智能体之前阅读它。** 它涵盖整个流程：为工作智能体分配独立的 git worktree 和工作区，通过提示词中的交付约定移交任务，确认命令确实已启动，跟进状态，从 git 收集结果，并行分派给多个工作智能体，以及清理。

即使你不读其他内容，也必须遵守其中四条规则：

- **使用已确认的原生交互入口。** 内置产品只有 omp、Kimi Code、pi、OpenCode 2.x、Hermes。任务投递按各产品正式接口执行；print／管道模式不是 TUI，无法支持用户持续观察和追问。缺少交互投递能力时保留错误与任务，不静默丢任务、盲目键入或用一次性输出冒充。
- **写入文件的工作智能体必须有自己的 git worktree。** 两个智能体共用一个检出目录，会互相覆盖，并破坏用户的工作树。
- **从 git 收集结果，而不是从屏幕收集。** 告诉工作智能体提交更改，然后读取 diff。屏幕只是一个矩形，顶部内容已经滚走。
- **向新窗格首次发送内容后，确认命令已离开提示符开始执行**——即执行上文所述的 Enter 被吞检查。

<a id="looking-around"></a>
## 查看概况

```bash
lyrel ls                    # 所有工作区：标签页、窗格、已连接的客户端
lyrel ws tree api           # 以树状显示一个工作区——标签页、拆分、窗格、cwd
lyrel pane ls               # 窗格及其工作区、标签页、cwd、存活标记
lyrel pane ls --all         # 额外包含孤立窗格：由服务器运行、但不属于任何工作区的窗格
lyrel agents                # 本机所有编程智能体及其状态
lyrel status                # 服务器 PID、运行时长、窗格数、构建信息、套接字
lyrel machine ls            # 本机及所有已链接的远程机器
lyrel events                # 流式输出服务器事件，每行一个，直到被中断
```

`lyrel agents` 值得了解：它会报告每个正在运行已识别编程智能体的窗格，其状态为 `idle` / `working` / `waiting` / `done`，旁边附带智能体自身的消息。如果你也是其中之一，你也会出现在列表中。当它发现某个正在运行的智能体缺少状态钩子或钩子已过期时，还会打印诊断信息——在 JSON 中对应 `diagnostics`——这正是智能体看起来停滞不动的原因。

为其中任意命令添加 `--json`，即可解析输出，而不必目视检查。`-q` 会抑制成功时的输出，但绝不抑制错误。

<a id="dont-break-the-users-session"></a>
## 不要破坏用户的会话

本机的窗格承载着用户的真实工作，其中一些还是正在执行任务的其他编程智能体。将所有不是你创建的内容视为只读：

- **绝不对不是你打开的窗格执行 `send` 或 `exec`。** 向另一个智能体的窗格，或用户正在输入的 shell 发送按键，会直接插入当前正在进行的操作中。操作窗格前先检查 `lyrel agents`。对 `--key` 更应加倍谨慎：一个误发的 `C-c` 就会终止别人的工作。
- **绝不对不是你创建的对象执行 `pane close` / `tab close` / `ws rm`。**
- **绝不执行 `pane close --orphans`。** 它会关闭本机所有被遗弃的窗格，而这些窗格仍可能在运行真实命令。这是用户的清理工具；可以告诉用户有这个选项，但不要替他们执行。
- **绝不执行 `server stop` 或 `server restart`。** 本机所有窗格都会随服务器一起终止，包括你自己的。如果服务器确实似乎卡死了，请说明情况，让用户决定。
- **清理你自己创建的内容。** 临时窗格用完后执行 `lyrel pane close %83`；它可以一次接受多个 ID。`ws rm` 会挂断工作区持有的窗格，因此删除临时工作区本身就足够了。真正会造成泄漏的是被中断的 `lyrel run`——其窗格会在没有任何引用的情况下继续运行，并出现在 `lyrel pane ls --all` 中。

<a id="remote-machines"></a>
## 远程机器

`-m <machine>` 会通过本地服务器已有的链接路由任意命令：

```bash
lyrel -m devbox ls
lyrel -m devbox run -- cargo test
```

名称可以匹配完整链接键（`me@devbox:22`），也可以只匹配主机名。CLI 不会发起新连接——如果链接已断开，或涉及跳板/代理链，它会说明情况；你应将此信息反馈给用户，由用户从 GUI 建立连接。

<a id="not-wired-up-yet"></a>
## 尚未接入的功能

`ws stop`、`machine connect` 和 `machine disconnect` 会退出，并提示尚未实现。不要围绕它们制定计划。

<a id="references"></a>
## 参考资料

- `references/delegation.md`——委派操作指南：worktree、交接、状态跟进、结果收集、并行分派、清理。每当准备让另一个智能体执行工作时，都应阅读。
- `references/commands.md`——在一张表中列出所有操作、子命令和选项，以及各自输出的 JSON 结构。当你需要使用上文未涉及的操作，或准备解析 `--json` 输出并想了解字段名称时，请阅读。