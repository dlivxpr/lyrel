# Delta 桌面端的工作树与更改入口

## Windows 实窗观察

- 底部将「项目 / 分支」与 `+增 −删` 分开。点击「项目 / 分支」打开项目菜单；点击增删数打开右侧 `Changes` 抽屉。
- 项目菜单的 `Work In` 区分 `Isolated Delta Worktree`、`Existing Local Checkout`、`Existing Git Worktree`。`Existing Git Worktree` 打开可搜索的子菜单，结果以工作树目录名为主行，分支和路径为副行。
- `Changes` 顶部的 `Since …` 菜单选择差异比较基准，实窗出现了 `origin/main`、`Last Commit`、`Last Turn`。切换比较基准不等同于选择终端所在检出目录。
- 此次只打开菜单和抽屉；没有切换检出、执行提交或移除工作树。未观察到与 lyrel 同类的提交消息输入框，未验证 Delta 在切换检出时的草稿行为。

## 官方契约与 lyrel 取舍

[Delta Worktrees](https://delta.dev/docs/concepts/worktrees) 说明 Delta 自管检出不是 Git 关联工作树；已有 Git 工作树是可选目标，线程开始使用检出后锁定位置。[Reviewing & Syncing Changes](https://delta.dev/docs/agents/review-and-sync) 说明 Changes 的比较基准切换只改变显示的差异。[Delta & Git](https://delta.dev/docs/concepts/delta-and-git) 说明 DeltaDB 记录文件编辑，但这不是 Git 提交，也不是提交消息草稿。

建议借鉴紧凑入口和可搜索的双行工作树列表，不照搬 DeltaDB、线程锁定或差异基准模型。lyrel 继续使用 Git 工作树，由当前终端窗格确定「更改」和底栏 Git 的上下文。提交草稿是否按工作树隔离是 lyrel 自己的产品决策，不归因于 Delta。

## 移除检出与分支

[Delta Worktrees](https://delta.dev/docs/concepts/worktrees#checkouts-a-delta-worktrees-files-on-a-machine) 明确区分：归档线程会移除 Delta 自管的本地检出，重开时可按历史重建；用户选用的已有检出不会被归档删除。[Threads](https://delta.dev/docs/agents/threads#archive-and-reopen-threads) 也明确说明归档不删除线程记录或 DeltaDB 工作树。

Windows 原生实测使用临时 Git 仓库、本地裸远程和干净的 `probe-linked` 关联工作树。通过 `Existing Git Worktree` 搜索并选择 `probe-linked` 后，底栏显示检出目录和分支，Git 工作树选择列表未显示磁盘删除操作。对临时项目执行 `Remove Project` 后，其入口从当前线程消失；前后两份 `git worktree list --porcelain`、`refs/heads` 和两个检出目录内的 `probe.txt` 内容完全一致。这个入口只解除项目关联，不移除 Git 工作树，也不删分支。没有操作其他项目或提交线程消息。

已确认的 lyrel 差异：新入口允许显式移除未被占用的关联工作树，不限创建来源；主工作树不在移除范围。有未提交改动时先说明影响并要求确认，快照失败则不移除。检出移除成功后沿用 lyrel 已有分支清理规则，由 Git 决定能否安全删除；确认时说明可能清理分支，并报告实际结果。
