# diri 与 lyrel 滚动阅读行为对比调研

- 日期：2026-11-10
- 调研对象：
  - diri：`https://github.com/cristicretu/diri`，commit `0b23e72080ca56b3cda795acb3ffa153eb6f75c8`（2026-11-09，v0.11.5）
  - lyrel：`/Users/dlivxpr/Documents/rscode/lyrel`，commit `5a38d0a9242761024335fe06263b040ddb3f5d74`（2026-11-10）
  - 相关依赖：`rio-vt = 0.5.29`（lyrel `Cargo.toml:326` 精确锁定，`Cargo.lock:9135-9137`）
- 本文中的代码行号以以上 commit / 版本为准。

## 1. 问题

同一个流式输出的智能体（agent）跑在 diri 里时，鼠标往上滚动查看前文，后续输出不会改写已滚上去的内容；跑在 lyrel 里时，只往上滑一点，正在查看的“前文思考内容”会被后面的输出覆盖。

## 2. 结论

diri 的稳定不是靠“修正模拟器的滚动偏移”，而是**把阅读态从活屏里剥离**：

1. 读视口离开底部时，clone 当前活屏（`held_live`），并把视口锚定到一个**绝对行号**（`anchor`）。
2. 后台继续解析输出、更新 live mirror；history 增长时只重算 `view_offset = live_start_row - anchor`，屏幕内容不动。
3. 阅读视口优先从 `held_live` 快照和已取回的历史行取数；读态下新到达的行用 `or_insert`，不覆盖旧行。
4. 只有回到底部（`view_offset == 0`）才释放快照和缓存。

lyrel 相反：滚轮只改 rio-vt 的 `display_offset`，绘制时每帧仍直接读 `term.grid[Line(row - display_offset)]`。rio-vt 0.5.29 已经能保证“追加行 / 整屏清屏”不移动滚动视图，但**仍留在活屏上的行是可变的**。Agent TUI 在流式输出时会用光标定位（CUP + EL）原地重绘最后一块活屏（spinner、当前消息、HUD），只往上滚一点时视口大部分仍是活屏，于是前文被重写。

diri 自己的性能文档也记录了这个动机（`diri/PERF.md:2317-2325`）：

> An absolute scroll anchor did not protect text still backed by the live grid. The desktop now captures one grid when scrolling away from live, and retains already fetched history rows ... Live damage still updates the authoritative mirror.

## 3. diri 的实现

### 3.1 终端在 daemon，客户端拿绝对行

- daemon 侧的模拟器是 alacritty：`diri/crates/diri-terminal-state/src/emulator.rs:27-30`。
- 输出更新携带绝对行起点：`diri/crates/diri-proto/src/grid.rs:434-439`

  ```rust
  pub struct GridHistory {
      /// Rows that have left history from its oldest end (evicted or cleared).
      pub origin: u64,
      /// Rows history holds: the absolute row the live grid starts at.
      pub rows: u64,
  }
  ```

- 客户端的 `ScrollbackViewport` 用绝对行寻址，完全不使用 alacritty 的 `display_offset`（diri 全仓 grep 无 `display_offset` 调用）。

### 3.2 阅读视口状态机

核心状态在 `diri/crates/diri-term/src/scrollback.rs`：

| 字段 | 位置 | 作用 |
|---|---|---|
| `view_offset: i64` | `scrollback.rs:150` | 距 live 边缘的行数 |
| `anchor: Option<i64>` | `scrollback.rs:170` | 阅读态下视口顶部锚定的绝对行 |
| `held_live: Option<GridBuffer>` | `scrollback.rs:184` | 离开 live 时的整屏快照 |
| `held_live_start: Option<i64>` | `scrollback.rs:185` | 快照对应的绝对行起点 |

字段注释（`scrollback.rs:162-171`）直接说明了为什么不能只用 `view_offset`：

> `view_offset` alone anchors the view to the *live edge*, so history growing under a scrolled reader slid the window forward onto rows it had not fetched ... Pinning content instead makes `view_offset` a derived value that grows as the live edge moves away.

### 3.3 关键代码路径

1. **离开 live 即快照**

   `diri/crates/diri-term/src/element.rs:181-186` 的 `set_view_offset` 在移动后立即调用 `hold_reading_view`；`scrollback.rs:774-782`：

   ```rust
   pub(crate) fn hold_reading_view(&mut self, buffer: &GridBuffer) {
       if self.find_source().is_none() && self.is_reading() && self.held_live.is_none() {
           self.held_live = Some(buffer.clone());
           self.held_live_start =
               (self.geometry_known || self.live_synced).then_some(self.live_start_row);
       }
   }
   ```

2. **读取时优先使用快照**

   `scrollback.rs:710-722` 的 `row_source`：`absolute_row` 落在 `held_live_start .. held_live_start + rows` 时返回快照 cell，否则才是 history cache 或当前 live buffer。

3. **live 更新只改派生 offset**

   `scrollback.rs:1198-1205`：读态下 `self.cache.entry(absolute).or_insert(row)`，新内容不覆盖已缓存行。

   `scrollback.rs:1214-1221`：

   ```rust
   // Output that scrolls lines into history moves the live edge, not the
   // content being read: hold the anchored row in place and let the
   // distance to live grow instead.
   if let Some(anchor) = self.anchor {
       self.view_offset = self.live_start_row.saturating_sub(anchor);
   }
   ```

4. **回到底部才释放**

   `scrollback.rs:783-798` 的 `release_reading_view`：丢弃 `held_live`、清空 cache/annotations、重置几何，下一次滚动重新捕获一屏。

### 3.4 其它入口

- 滚动条拖动：`set_offset` 只记录目标行，下一帧由 `set_view_offset` 应用（`diri/crates/diri-term/src/scrollbar.rs`）。
- 键盘、搜索跳转、`scroll_to_live`、`scroll_to_absolute` 都走 `set_view_offset` / `hold_reading_view`（`element.rs:1753-1761` 等）。
- alt screen 进入时强制回 live：`scrollback.rs` 的 `enter_alt_screen`。

## 4. lyrel 的实现

### 4.1 滚轮路径

`src/terminal/view.rs:1882-1935` 的 `on_scroll` 按路由分发；普通滚轮走 `smooth_scroll`：

```rust
// src/terminal/selection_scroll.rs:412-421
pub(super) fn smooth_scroll(&mut self, term: &mut Grid, delta: f32) -> bool {
    let (jump, fraction) = smooth_scroll_step(
        term.grid.display_offset(),
        self.fraction,
        delta,
        term.grid.history_size(),
    );
    if jump != 0 {
        term.scroll_display(Scroll::Delta(jump));
    }
    ...
}
```

`SelectionScroll` 的文档注释明确写着 `No grid is copied`（`selection_scroll.rs:17`）。滚动只改模拟器状态，不保存画面。

### 4.2 绘制路径

`src/terminal/element/snapshot.rs`：

```rust
// :231
display_offset = term.display_offset() as i32;
...
// :264-266
let line = Line(row as i32 - display_offset);
let source = &term.grid[line][..];
```

即：`display_offset == 0` 时画活屏；`> 0` 时上面几行画 history、下面仍画活屏。活屏单元格永远是最新解析结果，不会因为处于“阅读态”而冻结。

### 4.3 现有的 `held` 帧机制为什么不管用

`src/terminal/element.rs:539-543`：

```rust
let held = matches!(
    &built,
    Err(SnapshotUnavailable::Synchronized | SnapshotUnavailable::AwaitingSize)
) || (built.is_err() && previous.as_ref().is_some_and(|snap| snap.held));
```

`GridSnapshot.held` 只在同步输出（`?2026h`）和等待 resize 时复用上一帧；它不是“阅读态快照”。输出到达时 `PaneEvent::Wakeup` 只调用 `cx.notify()`（`src/terminal/view/events.rs:17-35`），下一次 paint 会重新 `build_grid` 并读到新的活屏。

## 5. 对比表

| 维度 | diri | lyrel |
|---|---|---|
| 终端状态位置 | daemon（alacritty）+ 客户端绝对行协议 | 客户端进程内 rio-vt |
| 视口坐标 | 绝对行 `anchor` + 派生 `view_offset` | rio-vt `display_offset`（距底部行数） |
| 新输出追加 | `view_offset = live_start_row - anchor`，内容不动 | rio-vt `display_offset += positions`，内容不动 |
| 活屏原地重绘 | `held_live` 快照屏蔽 | 直接读 `term.grid`，立即变化 |
| 读态历史行 | 已取回行 `or_insert`，不覆盖 | 已滚入 history 的行不可变 |
| 回到底部 | 释放快照与缓存 | 无需释放 |
| 滚动执行 | 客户端 viewport + 按需取历史行 | `term.scroll_display` |

## 6. 最小复现（rio-vt 0.5.29）

lyrel 锁定 `rio-vt = 0.5.29`。为确认“偏移是否会被新输出推走”，用同版本写了最小程序（20 列 × 5 行，scrollback 1000）。

```rust
use rio_vt::crosswords::grid::{Dimensions, Scroll};
use rio_vt::crosswords::pos::Line;
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::VoidListener;
use rio_vt::performer::handler::Processor;

fn feed(term: &mut Crosswords<VoidListener>, bytes: &[u8]) {
    Processor::default().advance(term, bytes);
}

fn visible(term: &Crosswords<VoidListener>) -> Vec<String> {
    let off = term.grid.display_offset() as i32;
    (0..term.screen_lines())
        .map(|r| {
            term.grid[Line(r as i32 - off)]
                .inner
                .iter()
                .map(|c| c.c())
                .collect::<String>()
                .trim_end_matches('\0')
                .trim_end()
                .to_string()
        })
        .collect()
}

fn main() {
    let mut t = Crosswords::new(
        CrosswordsSize::new(20, 5),
        Default::default(),
        VoidListener,
        0.into(),
        0,
        1000,
    );
    feed(&mut t, b"AAA0\r\nAAA1\r\nAAA2\r\nAAA3\r\nAAA4\r\nBBB0\r\nBBB1\r\nBBB2\r\nBBB3\r\nBBB4\r\n");
    t.scroll_display(Scroll::Delta(2));
    println!("scrolled 2; offset={}\n{:?}", t.grid.display_offset(), visible(&t));

    // 1) 光标定位原地重绘活屏第 2 行
    feed(&mut t, b"\x1b[2;1H\x1b[2KREWRITTEN");
    println!("after in-place repaint; offset={}\n{:?}", t.grid.display_offset(), visible(&t));

    // 2) 整屏擦除
    feed(&mut t, b"\x1b[2J\x1b[H");
    println!("after ED2; offset={} history={}\n{:?}", t.grid.display_offset(), t.grid.history_size(), visible(&t));
}
```

实测输出：

```text
scrolled 2; offset=2
["AAA4", "BBB0", "BBB1", "BBB2", "BBB3"]
after in-place repaint; offset=2
["AAA4", "BBB0", "BBB1", "REWRITTEN", "BBB3"]
after ED2; offset=6 history=10
["AAA4", "BBB0", "BBB1", "REWRITTEN", "BBB3"]
```

结论：

- **纯追加**：滚动后再追加 4 行，`display_offset` 从 2 变 6，视口顶部仍是 `AAA4`，内容不动。rio-vt 的补偿在 `src/crosswords/grid/mod.rs:400-411`。
- **整屏清屏**：`ED(2)` 走 `clear_viewport`，仍通过 `scroll_up` 补偿，`offset` 从 2 变 6，视口内容不动。
- **光标定位原地重绘**：`\x1b[2;1H\x1b[2K...` 直接改写视口中的活屏行，内容立即变化。这是 lyrel 被“覆盖”的机制。

## 7. 根因判定

### 7.1 主因：活屏行被 Agent 原地重绘

流式 agent TUI 的典型渲染方式：

- 已完成的消息/思考块提交到终端 scrollback；
- 当前正在生成的消息、thinking、spinner、HUD 仍留在活屏；
- 每个 token/帧用 `CUP` + `EL`/`ED` 局部重绘这些行。

lyrel 只往上滚 1–N 行时，视口 = N 行 history + `screen_rows - N` 行活屏。N 越小，活屏占比越大，重绘就越明显。往上滚满一屏后视口全部是 history 行，应该稳定——这是最快的现场判别方法。

### 7.2 其它会“整屏跳回底部”的情况

如果观察到的不是局部变化，而是整个视口瞬间回到最新输出，检查：

1. **Agent 发送 `CSI 3J`（清 scrollback）**：rio-vt `clear_history` 会把 `display_offset` 置 0（`src/crosswords/grid/mod.rs:529-536`）。部分 TUI 在 resize、重建 transcript、切换 session 时会发 `\x1b[2J\x1b[3J\x1b[H`。
2. **Agent 打开了 mouse reporting**：普通滚轮被转发给 agent，不再走 lyrel 本地 scrollback（`src/terminal/view.rs:2058-2073`）。Shift+wheel 才强制本地滚动。omp 的 `tui.mouse` 默认是 `false`，所以默认不走这条；若用户开启，滚轮行为由 agent 自己决定。
3. **进入 alternate screen**：alt screen 没有 scrollback，视口回到活屏是预期行为。

### 7.3 现场区分方法

| 现象 | 判定 |
|---|---|
| 只滚 1–2 行时局部文字变化，滚满一屏后稳定 | 7.1 活屏重绘，lyrel 缺少阅读快照 |
| 滚动后整个视口瞬间回到底部 | 查 `CSI 3J`、RIS、mouse reporting |
| Shift+wheel 时稳定，普通滚轮无效/异常 | mouse reporting 路由 |
| 输出只有追加、没有 TUI 重绘时也漂移 | 需要检查更底层（当前 rio-vt 0.5.29 实测不会） |

## 8. 修复方案（lyrel）

### 8.1 数据结构

新增阅读快照：

```rust
struct ReadingView {
    /// 视口顶部绝对行，离开 live 时记录
    top_row: i64,
    /// 快照覆盖的绝对行起点 = lines_evicted + history_size（捕获时）
    live_start_row: i64,
    /// rows × cols 的 RenderCell / Cell 拷贝
    cells: Vec<RenderCell>,
    cols: usize,
    rows: usize,
}
```

放在 `TerminalView` 或 `SelectionScroll` 均可，只在 `display_offset > 0` 时存在。

### 8.2 绘制规则

窗口行 `i` 的绝对行 `abs = top_row + i`：

1. `abs < live_start_row`：从 `term.grid` 的 history 读（已滚出的行不可变）。
2. `live_start_row <= abs < live_start_row + rows`：从快照读，**不要读当前活屏**。
3. 回 live（`display_offset == 0`）：丢弃快照，按现有逻辑画活屏。

快照的绝对基址用 `term.grid.lines_evicted() as i64 + term.grid.history_size() as i64`；`display_offset` 的变化由 rio-vt 的补偿维护，也可以自己按锚点重算，避免 `3J`/回绕时漂移。

### 8.3 失效条件

- 回到底部：释放。
- resize / reflow：丢弃并按新宽度重新捕获（或回 live）。
- 进入 alt screen：丢弃并回 live。
- `CSI 3J` / RIS 清历史：历史没了，快照可以保留，但 anchor 要 clamp 到最老行；超出部分按 diri 的 `max_offset` 截断。
- 历史被 evict 到快照范围之外：clamp，必要时回 live。

### 8.4 成本与风险

- 内存：只有滚动时多一屏 cell。diri 的 160×50 基准中捕获/释放约 1.4 µs（`diri/PERF.md:2325-2327`）。
- 风险：选择框、光标、IME 锚点、链接命中和图形 placement 的坐标要改成基于快照坐标；这是主要实现工作量。
- 不能通过“停止解析输出”来冻结：grid 必须继续更新，冻结的只是呈现。

## 9. 验证清单

1. 在 lyrel 中运行流式 agent，滚 1 行 → 观察活屏区域是否仍被重写。
2. 滚满一屏 → 历史区域应稳定（当前行为下也应如此）。
3. Shift+wheel 强制本地 scrollback，排除 mouse reporting。
4. 加阅读快照后重复 1，历史/活屏交界处应稳定；回到底部后应恢复跟随。
5. 回归：resize、alt screen、`3J`、滚动条拖动、搜索跳转、选择复制。

## 10. 证据边界

- diri / lyrel 的实现结论来自上述 commit 的源码与行号，未修改任何仓库。
- rio-vt 行为来自 0.5.29 源码 + 第 6 节可复现实验。
- 未在 lyrel GUI 中驱动真实 agent 做端到端复现；“活屏重绘是主因”是基于代码路径和 emulator 实验的推断（与 diri 的 `PERF.md` 记录一致）。7.2 的 `3J` / mouse reporting 是替代路径，需要按 7.3 现场区分。

## 附录：行号索引

diri：

- `diri/crates/diri-proto/src/grid.rs:434-439` — `GridHistory`
- `diri/crates/diri-terminal-state/src/emulator.rs:27-30` — alacritty `Term`
- `diri/crates/diri-term/src/scrollback.rs:150-196` — viewport 状态字段
- `diri/crates/diri-term/src/scrollback.rs:710-722` — `row_source` 优先快照
- `diri/crates/diri-term/src/scrollback.rs:774-782` — `hold_reading_view`
- `diri/crates/diri-term/src/scrollback.rs:783-798` — `release_reading_view`
- `diri/crates/diri-term/src/scrollback.rs:1198-1205` — 读态 `or_insert`
- `diri/crates/diri-term/src/scrollback.rs:1214-1221` — anchor 重算 offset
- `diri/crates/diri-term/src/element.rs:181-186` — `set_view_offset` 触发快照
- `diri/crates/diri-term/src/element.rs:1753-1761` — `scroll_to_live`
- `diri/PERF.md:2317-2325` — “Stable reading during streaming redraws”

lyrel：

- `Cargo.toml:326` / `Cargo.lock:9135-9137` — `rio-vt = 0.5.29`
- `src/terminal/selection_scroll.rs:17` — “No grid is copied”
- `src/terminal/selection_scroll.rs:412-421` — `smooth_scroll`
- `src/terminal/element/snapshot.rs:231` — 读取 `display_offset`
- `src/terminal/element/snapshot.rs:264-266` — 逐行读 `term.grid`
- `src/terminal/element.rs:539-543` — `held` 仅同步输出/等待 resize
- `src/terminal/view.rs:1882-1935` — `on_scroll`
- `src/terminal/view.rs:2058-2073` — `wheel_route`
- `src/terminal/view/events.rs:17-35` — `Wakeup` 只 notify

rio-vt 0.5.29：

- `src/crosswords/grid/mod.rs:400-411` — `scroll_up` 补偿 `display_offset`
- `src/crosswords/grid/mod.rs:529-536` — `clear_history` 将 `display_offset` 置 0
- `src/crosswords/mod.rs:1286-1310` — `scroll_up_relative`：offset 在历史上限被 clamp 时标记全屏 damage
