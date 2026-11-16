---
name: lyrel-release
description: >-
  准备、打包、验证或发布 Lyrel stable 版本时使用，覆盖版本锁文件、发行资产、
  checksums、ConPTY、打包二进制冒烟及发布前后核验。
  检查版本或草稿不授权创建标签、推送、公开发布或替换已发布资产。
---

# Lyrel 发行流程

路径均相对仓库根。开发环境和 ConPTY 准备读 `.agent/skills/lyrel-development/SKILL.md`；GitHub 授权及交付读 `.agent/skills/lyrel-github/SKILL.md`。当前目标与资产契约从 `.github/workflows/release.yml`、其调用的 `.github/workflows/release-build.yml` 和打包脚本读取，不固定历史资产数或所谓最新版本。

## 版本与草稿

1. 确认目标版本、提交、发布范围及用户授权；保留既有用户改动。项目仅发布 stable，Release 标题为 `v<version>`，发布说明正文使用简体中文。
2. 提升 `workspace.package.version` 时执行 `cargo update --workspace --offline`，将 `Cargo.toml` 与对应 `Cargo.lock` 版本变更放入同一获准提交；不能仅改 manifest 导致 `--locked` 拒绝。
3. 创建或推送 `v<version>` tag 必须在授权范围内，且 tag 与目标提交及 workspace 版本一致；需要 `target_commitish` 的接口使用真实目标提交，不把版本 tag 当作提交 SHA。
4. 由当前工作流组装草稿。草稿存在或工作流成功不等于可以公开发布；核对同一提交的 CI、构建、完整资产、校验和和中文说明后才进入发布判断。

## 发布前验证

- 从目标草稿取得完整资产清单，对每个资产核对大小和 `checksums.txt` 中的 SHA-256；服务端提供 digest 时同时核对。不以抽样或历史固定数量代替全量检查。
- Windows 包必须含匹配的 `conpty.dll`、`OpenConsole.exe`、MIT 通知及契约要求的清单；核对架构、版本和摘要。打包须显式暂存并断言这些文件，不能依赖可能被 Cargo 缓存的 `build.rs` 副作用。
- 在隔离目录解包并运行真实分发二进制，不用本地 `target/` 产物代替。Windows CLI/server 的版本、安装器 `X.Y.Z.0` 文件版本应与目标版本一致。
- Linux server 在匹配 libc/依赖的 Docker Linux 环境中冒烟；Docker 不可用或目标要求 WSL 时记录回退原因。核对静态链接契约及 AArch64 的静态 ELF64，不只依据文件名或编译目标宣称成功。
- 核对 macOS bundle 的 `CFBundleVersion` 与 `CFBundleShortVersionString`，检查本次实际签名状态；不能将历史 ad-hoc/unsigned 状态当作当前事实。
- 记录实际安装、原生窗口、中文 IME、SSH 和在线更新是否验证。只检查归档、版本或校验和不证明这些路径通过。
- 发布前保持 draft。缺资产、校验失败、运行失败或验收前提缺失必须如实处理，不能为完成清单先改成非草稿。

## 公开发布与回读

只有用户授权覆盖具体版本和公开发布操作时才执行；否则交付草稿与验证结果。发布后回读 tag、目标提交、draft/prerelease 状态及预期 Latest 标记，重新确认资产清单与校验和。不得通过重跑覆盖已发布资产。

## 清理与历史陷阱

- 停止本次测试 daemon/app，只清理自建且不再需要的隔离配置和临时文件，保留必要证据与构建资产；分支、镜像和磁盘操作按 GitHub Skill 的授权边界处理。
- Unix PowerShell 路径列表使用 `[IO.Path]::PathSeparator`，不要固定拼接 `;`。Windows 跨盘测试夹具用 `std::fs::copy`，不能依赖 rename/hard-link。
- macOS cross-daemon graphics-budget 的历史时序问题应核对 viewport 发布和事件泵顺序；不把重试成功当作证明竞态已修复。
