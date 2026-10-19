<a id="delegating-work-to-another-coding-agent"></a>
# 将工作委派给另一个编程智能体

窗格中的工作智能体是一个真实的智能体会话，用户可以观察、中断并接管它——这正是窗格优于 API 调用的地方。本操作指南按顺序介绍一次委派的完整流程：

1. [提供工作场所](#1-a-place-to-work)——独立的工作树和工作区
2. [交付任务](#2-handing-over-the-task)——使用交互模式，在提示词中约定交付要求
3. [确认已经启动](#3-prove-it-started)——已启动的工作智能体实际上没有运行的三种隐蔽情况
4. [等待并照看](#4-wait-and-babysit)——状态、`--changed`、回应提示
5. [通过 git 收集成果](#5-collect-through-git-not-the-screen)——屏幕用于诊断，不是交付物
6. [清理](#6-clean-up)

[并行分派](#running-several-workers)建立在相同的六个步骤之上，每个工作智能体都分别执行这些步骤。

<a id="1-a-place-to-work"></a>
## 1. 提供工作场所

**需要写入文件的工作智能体必须拥有独立的 git 工作树。** 两个智能体共用一个检出目录，或者智能体使用用户正在编辑的检出目录，都会互相干扰：尚未写完的文件出现在彼此的差异中，构建相互争抢资源，`git status` 也不再具有明确意义。整个并行智能体工具领域最终都采用了同一个方案：一个任务、一个工作树、一个分支。

```bash
REPO=/path/to/repo
WT=/tmp/agent-wt/parser-tests
git -C "$REPO" worktree add "$WT" -b agent/parser-tests
read -r WS PANE < <(lyrel new --json "$WT" \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["id"], "%%%d" % d["pane"])')
```

使用 `lyrel new` 而不是 `split`，原因与使用工作树相同，只是层级更高：独立工作区可以避免工作智能体干扰用户的窗口布局。如果只有一个短期运行、且用户希望观察的工作智能体，拆分自己的窗格也没问题；但拆分到三层时，窗口就难以使用了。

只有在工作智能体不会写入文件时，才可以省略工作树：例如研究问题、阅读代码、提供第二意见。对于这些任务，在当前检出目录中使用 `split` 即可。如果用户明确要求工作智能体操作其正在使用的检出目录，应说明风险，然后按要求执行。

<a id="2-handing-over-the-task"></a>
## 2. 交付任务

将任务作为参数传给**交互式**命令——绝不要使用 `-p`：

```bash
TASK="为 src/lexer.rs 中分词器的边界情况添加测试。
完成后，提交到当前分支，并在提交说明中写明
做了什么以及验证了什么。不要推送。"
lyrel send "$PANE" "omp \"$TASK\"" --enter
```

为什么要使用交互模式：两种模式都会运行一轮后停止，因此区别不在于工作智能体做什么，而在于它工作时其他人能看到什么。交互模式会绘制 TUI，因此窗格会实时显示工作智能体的推理和工具调用；这正是用户观察的内容，也是 `capture --plain` 读回的内容。`-p` 是管道模式：它不绘制任何内容，在这一轮结束前，窗格屏幕始终**空白**——看起来与工作智能体卡死完全一样——而且结束后会话就消失了，无法继续追问。把使用 `-p` 的工作智能体放进窗格，就丢掉了使用窗格的唯一理由。如果你只想得到一个字符串形式的答案，而且不需要任何人观察过程，应使用 `lyrel run` 或你自己的 Bash 工具，而不是窗格。

每个任务提示词都应包含两项内容：

- **交付约定。** “完成后提交到当前分支”能把结果转化为 git 可以完整交付给你的内容——参见[第 5 步](#5-collect-through-git-not-the-screen)。否则，你只能阅读屏幕末尾的 40 行，然后猜测结果。
- **边界。** “不要推送”“仅在此目录内操作”“删除前先询问”——工作智能体不会继承你的任何上下文，只会收到它的提示词。

<a id="3-prove-it-started"></a>
## 3. 确认已经启动

刚刚启动的工作智能体可能因三种相互独立的原因而悄无声息地没有运行，以下检查可以分别发现它们：

| 故障 | 你看到的现象 | 检查方法 |
|---|---|---|
| 新启动的 shell 吞掉了回车 | 命令停留在提示符处，始终没有执行 | `capture --plain \| tail -3`；如果仍停留在提示符处 → `lyrel send "$PANE" --enter` |
| `lyrel agents` 报告的是过时状态 | 卡在提示符处的窗格仍可能显示 `working` | 以屏幕最后几行为准，而不是状态 |
| `lyrel procs` 显示 `nothing running` | 看起来像工作智能体已经终止 | 它并没有终止——`procs` 看不到智能体；此处忽略它 |

对任何刚创建的窗格执行首次 `send` 后，都必须进行第一项检查。在这里花三十秒，胜过对一个从未开始工作的智能体执行 `wait`，然后等到 900 秒超时。

<a id="4-wait-and-babysit"></a>
## 4. 等待并照看

```bash
lyrel wait "$PANE" --until waiting,done --changed --timeout 1800
```

<a id="what-the-states-mean"></a>
### 各状态的含义

| 状态 | 窗格的情况 |
|---|---|
| `working` | 正在执行一轮任务 |
| `waiting` | **已停止，需要你介入**——例如权限提示或提问 |
| `done` | 已完成本轮任务 |
| `idle` | 智能体尚未开始一轮任务 |
| `free` | 没有智能体：前台命令已退出 |
| `no-agent` | 没有任何组件在此报告状态——可能是普通 shell，也可能是未安装钩子 |
| `exit` | 窗格已不存在；无论你是否指定此状态，它都会结束所有等待 |

默认值是 `--until waiting,done,exit`，因为这三个状态都表示“又轮到你了”。`idle` 是智能体对*自身*状态的描述——正在运行构建的窗格是 `no-agent`，绝不是 `idle`，因此 `--until idle` 绝不能用于询问“命令是否已经结束”；应使用 `free`。混用智能体状态与 `free` 是安全的：只有在未先匹配到任何具名智能体状态时，才会检查 `free`。

<a id="--changed-on-every-wait-that-follows-a-send"></a>
### 每次 send 之后的 wait 都要使用 `--changed`

状态是**持续值，而不是事件**：`done` 会一直保持到下一轮开始。如果在 `send` 后立即执行 `wait`，工作智能体甚至还没读取输入，`wait` 就可能直接返回*上一轮*的 `done`；你随后读到过时的屏幕内容，误以为它失败了。`--changed` 会拒绝接受窗格原本就处于的状态。每次向窗格输入之后的等待都需要它；JSON 中的 `stale` 标志会告诉你它何时起了作用。如果等待之前*没有进行任何操作*——例如[并行分派](#running-several-workers)中的成果收集轮询——则必须不使用它。

<a id="the-babysit-loop"></a>
### 照看循环

`waiting` 表示工作智能体停下来等待人工介入。在你能够处理时，就由你来回应：

```bash
lyrel capture "$PANE" --plain | tail -20   # 它在询问什么？
lyrel send "$PANE" --key down --key enter  # 回应菜单或权限提示
lyrel send "$PANE" '是的，使用现有的测试夹具文件' --enter   # 回答问题
```

然后继续等待。对于任务边界已经涵盖的问题，可以自行回答；任何超出边界的事项——破坏性操作、范围变更、凭据——都应报告给用户，并附上窗格 ID，方便用户亲自查看。无法安全代答时，就由用户接管工作智能体；这种交接是功能，不是失败。

要停止失控的工作智能体，使用 `lyrel send "$PANE" --key C-c`——将 `^C` 作为文本输入，只会发送两个无害字符。

`wait` 的退出码：`0` 表示已达到你要求的某个状态，`124` 表示超时（遵循 `timeout(1)` 惯例——表示“尚未达到”，可与故障区分），`1` 表示窗格已终止。

<a id="5-collect-through-git-not-the-screen"></a>
## 5. 通过 git 收集成果，而不是屏幕

第 2 步中的交付约定会在这里发挥作用：

```bash
git -C "$WT" log --oneline main..HEAD    # 它声称交付了什么
git -C "$WT" diff main...HEAD            # 实际变更
git -C "$WT" status --short              # 是否留下未提交的内容
```

内容完整、不因屏幕宽度而折行，也不会因滚动而丢失——而且在任何一个字节进入用户分支之前，都可以审查。你就是合并的把关点：阅读差异，根据任务需要运行测试，然后合并或报告。

`capture --plain | tail` 用于*诊断*——它卡在哪里、刚刚输出了什么——而不是收集成果。屏幕只是一个 120 列宽的矩形，有用的部分通常早已从顶部滚出。

如果工作智能体已完成却没有提交任何内容，应从屏幕中查明原因；这是屏幕末尾内容成为交付物的唯一情况。

<a id="6-clean-up"></a>
## 6. 清理

```bash
lyrel ws rm "$WS"                          # 挂断工作区内的窗格
git -C "$REPO" worktree remove "$WT"      # 工作区不干净时会拒绝删除——这是有意的保护
git -C "$REPO" branch -D agent/parser-tests   # 合并或决定不采用之后执行
```

`worktree remove` 拒绝执行，意味着那里还有未提交的工作——作出任何决定之前先查看这些内容，并询问用户，而不是用 `--force` 删除尚未阅读的变更。如果工作智能体是通过 `split` 打开的，则只需执行 `lyrel pane close "$PANE"`。

<a id="running-several-workers"></a>
## 运行多个工作智能体

并行分派就是对每个工作智能体执行上述六步——**每个都有独立的工作树和分支**——再加上一个不会被任何单个卡住的工作智能体阻塞的成果收集循环：

```bash
: > /tmp/agent-workers
for task in parser lexer codegen; do
  WT=/tmp/agent-wt/$task
  git -C "$REPO" worktree add "$WT" -b "agent/$task"
  read -r WS PANE < <(lyrel new --json "$WT" \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["id"], "%%%d" % d["pane"])')
  lyrel send "$PANE" "omp \"为 $task 添加测试。完成后提交到当前分支；不要推送。\"" --enter
  echo "$task $PANE $WT $WS" >> /tmp/agent-workers
done
# 开始等待之前，确认每个工作智能体都已启动（第 3 步）

cp /tmp/agent-workers /tmp/agent-pending
while [ -s /tmp/agent-pending ]; do
  : > /tmp/agent-still
  while read -r task PANE WT WS; do
    lyrel wait "$PANE" --until waiting,done --timeout 120; rc=$?
    if [ $rc -eq 0 ]; then
      : # done → 收集成果（第 5 步）；waiting → 照看（第 4 步），然后重新入队
    elif [ $rc -eq 124 ]; then
      echo "$task $PANE $WT $WS" >> /tmp/agent-still   # 尚未完成——稍后再来
    else
      echo "$task：窗格 $PANE 已不存在" >&2             # 1：已终止；无需重新入队
    fi
  done < /tmp/agent-pending
  mv /tmp/agent-still /tmp/agent-pending
done
```

关键就在于每个工作智能体的超时时间要短：如果按顺序对每个工作智能体执行一次长时间的 `wait`，第一个卡住的工作智能体就会让你无法看到后面所有工作智能体的情况。以 120 秒为周期轮询，可以让你不断巡视——收集已完成的成果、回应受阻的工作智能体，并向用户报告连续三轮毫无进展的那个。

这里与第 4 步不同，不使用 `--changed`，这是有意的：如果一个工作智能体在你等待*另一个*工作智能体时进入了 `done`，那么轮到它自己的 `wait` 开始时，它就已经处于该状态，`--changed` 会拒绝接受它——每一轮都如此，永无止境。每个窗格只运行一轮任务，因此持续存在的 `done` 就是本轮的完成状态。这样做的代价在另一端：回应处于 `waiting` 的工作智能体后，在重新入队之前，先执行 `lyrel wait "$PANE" --until working --changed --timeout 30`，否则下一轮又会把同一个提示交给你。

不要构建这样的协作方式：工作智能体彼此通信，或将各自的分支相互合并。应保持星形拓扑——你分派互不重叠的任务，每个工作智能体交付到自己的分支，所有差异都依次经过你审查。两个工作智能体分支之间涉及多个部分的冲突，由你在合并时解决；这也正是任务一开始就不应重叠的原因。

<a id="when-a-worker-never-moves"></a>
## 工作智能体始终没有动静时

如果 `wait` 超时，而 `lyrel agents` 显示的状态始终不变，几乎总是因为智能体的状态钩子缺失或过时——工作智能体本身没问题，只是无法报告状态。`lyrel agents` 在能够发现缺失时会指出对应智能体，`lyrel doctor` 则会报告每个智能体的钩子状况。钩子通过 GUI 的**设置 → 智能体**安装；应告知用户，而不是尝试自行安装。

作出任何结论之前，先检查它是否仍在活动。同一套钩子会在每次工具调用时发出一行 OSC 777，而**不带** `--plain` 的 `capture` 可以显示这些内容——这是原始字节优于渲染屏幕的少数场景之一：

```bash
lyrel capture "$PANE" | grep -c 'tool-complete'   # 数量上升 = 仍在运行并工作
```

有两项检查*不能*回答这个问题：一是 `lyrel procs`，对于内部有活跃智能体的窗格，它仍会报告没有运行中的程序；二是空白的 `capture --plain`——误用 `-p` 启动的工作智能体在整轮任务中都不会绘制任何内容，即便底层事件流十分活跃。

<a id="a-stuck-working-can-be-an-aborted-turn"></a>
### 卡在 `working` 可能是某轮任务意外中止

钩子报告的是每轮任务的边界，因此，如果某一轮因 API 错误、连接中断等原因终止，却没有报告边界，状态就会永远停留在 `working`，而你的 `wait` 会一直等待，无法察觉。实际情况要看屏幕：捕获末尾内容，检查输入框上方是否有错误行（`API Error: Connection closed mid-response` 及类似错误）。

恢复能力正是让工作智能体运行在交互模式中的原因：会话仍在提示符处存活，保留全部上下文以及所有未完成的编辑。告诉它继续工作——

```bash
lyrel send "$PANE" '你的上一轮因 API 错误而中断。请继续，并按要求完成任务。' --enter
```

——然后继续等待。

<a id="tail-enough-lines-to-see-the-spinner"></a>
### 保留足够多的末尾行，以便看到进度动画

TUI 会将输入框固定在屏幕**底部**，因此过短的末尾输出——例如 `tail -5`——无论工作智能体处于空闲状态，还是已经深入编辑了三个文件，显示的都是空白输入框：真正能区分二者的是十几行上方的进度动画或耗时行，而过短的末尾输出会将其截掉。只要问题是“它有没有在做事”，就应使用 `tail -15` 或更多行，并观察进度动画，而不是提示符。“底部看起来像提示符”对于 *shell* 窗格来说是有用证据（第 3 步中的启动检查）；但对于 TUI 窗格，屏幕本来就始终是这个样子。