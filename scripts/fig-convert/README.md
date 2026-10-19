<a id="fig-convert--fig-autocomplete-specs--lyrel-completion-signatures"></a>
# fig-convert — Fig 自动补全规范 → lyrel 补全签名

lyrel 针对各命令的补全（带描述的标志、子命令和参数）由签名数据驱动，这些数据生成自 [Fig 的自动补全规范集][fig]（采用 MIT 许可证，由社区维护，涵盖数百个命令）。这是**构建时转换器**；运行时使用这些数据的模块是 `src/terminal/signature.rs`。

本次固定来源为 npm 发布包 [`@withfig/autocomplete@2.692.3`](https://registry.npmjs.org/@withfig%2fautocomplete/2.692.3)，需要 Node.js 20+。包的 SHA-512 integrity 为 `sha512-zxN7K8W+qWGc2ejWksd92xwAyCZ0faHvnk3c7jUoWkkk5e5iQf3/4mOLheFvYLoUWR5a36571VJ1DdyxmAWJZA==`。该版本是本次核对的 npm `latest`，不代表每份命令规范会随对应 CLI 实时更新。

随包 `LICENSE` 与上游仓库标明 MIT，版权为 `2021 Hercules Labs Inc. (Fig)`；npm 的 `license` 元数据却标为 ISC，二者不一致。本仓将随包许可证原文保存在 `assets/completions/LICENSE-Fig.txt`，不依据单个元数据字段改写原始许可与版权声明。

<a id="why-a-converter-and-not-the-js-engine"></a>
## 为什么使用转换器（而不是 JS 引擎）

Fig 规范使用 TypeScript 编写，并以**预编译**的 ESM 形式发布到 npm。每份规范都通过 `export default` 导出一个对象，其*静态*结构（子命令、选项、参数、描述、静态生成器 `script`）正是补全菜单所需的内容。真正动态的部分只有函数——生成器的 `postProcess`、`custom`、`filterTerm`、`generateSpec`——它们需要 JS 运行时才能求值。

我们提前进行转换：将每份规范运行一次，把静态结构保存为 JSON 快照，并丢弃函数。**运行时不需要 JS 引擎。**（另一种方案是嵌入 QuickJS 来实时执行这些函数；我们不需要这样做。）动态值补全（`git checkout <branch>`）之后可以通过运行生成器的静态 `script` 并按换行符拆分 stdout 来恢复——这些 `script` 已经被提取保存；只有执行器尚待实现。

<a id="regenerate"></a>
## 重新生成

在 `scripts/fig-convert/` 内执行；只安装转换需要的生产依赖，不运行包的 lifecycle scripts：

```bash
# 1. 取得固定版本及其运行转换所需的依赖（均留在被忽略的 package/ 内）。
npm pack @withfig/autocomplete@2.692.3 --ignore-scripts
tar xzf withfig-autocomplete-2.692.3.tgz
npm install --prefix package --omit=dev --ignore-scripts --no-package-lock

# 2. 重新生成当前随附的全部命令，不用手工维护另一份命令名单。
node --input-type=module -e '
import { readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
const commands = readdirSync("../../assets/completions")
  .filter(name => name.endsWith(".json")).map(name => name.slice(0, -5)).sort();
const result = spawnSync(process.execPath, ["convert.mjs", "--build-dir", "package/build",
  "--out", "../../assets/completions", ...commands], { stdio: "inherit" });
process.exit(result.status ?? 1);
'
```

本次实际转换为 **97/97**，没有命令缺失或 `loadSpec` 失败。转换器会继续处理单项失败后的命令，因此重新生成时仍须核对汇总及 `✗`／`FAILED` 输出，不能仅凭退出码为 0 判断覆盖完整。

lyrel 自有补充保存在 `local-overrides.json`：34 处说明文案按子命令／选项名称定位；`ssh`、`scp`、`sftp`、`rsync` 保留 SSH config／Include／known_hosts 的静态主机生成器，传输命令同时保留本地路径补全。转换器在静态规范归一化后应用这些补充；不要手改生成 JSON，否则下一次再生成会丢失产品行为。目标命令节点缺失时会报告该项转换失败，需要随发布包升级一并复核。

`convert.mjs` 会执行每个 `<cmd>.js`，遍历对象图并仅保留静态字段，解析 `loadSpec` 引用（例如，`docker compose` 会接入 `docker-compose` 规范），然后写出压缩后的 `assets/completions/<cmd>.json`。每个命令都独立转换：某份规范抛出异常时，会报告（`✗`）并跳过它，不会中止整批转换。`index` 是汇总导出模块，不应作为命令输入。本次验收覆盖当前随附的 97 个命令，不扩写为发布包中其他规范均已验证。一次运行中可以传入任意多个命令名称。

<a id="output-schema"></a>
## 输出模式

与 `src/terminal/signature.rs` 中的 serde 模型一一对应：

- 根对象：`{ name, description, options[], args[], subcommands[] }`
- 子命令：`{ names[], description, hidden, options[], args[], subcommands[] }`
- 选项：`{ names[], description, args[], required, repeatable, hidden }`
- 参数：`{ name, description, optional, variadic, template[], suggestions[], generators[] }`
- 建议项：`{ names[], description }`
- 生成器：`{ script[] }`（仅保留静态 shell 命令；丢弃 `postProcess`）

<a id="scope--rollout"></a>
## 范围与分发

签名**从磁盘读取**，而非嵌入二进制文件：`signature::spec_source` 会定位 `completions/` 目录（位于 macOS `.app` 包的 `Resources` 内、Windows 可执行文件旁，或供 `cargo run`/测试使用的源码树内 `assets/completions`），并按命令名称延迟加载 `<cmd>.json`。打包脚本复制 JSON 和 `LICENSE-Fig.txt` 到各自分发目录；Windows 的实际暂存及在 Linux 上运行的 macOS 资源暂存均已核对，不将后者算作 macOS 构建／签名验证。

源码树当前随附 97 个日常命令（版本控制、容器/k8s、语言包管理器、云 CLI、shell/系统管理工具）。本次使用固定发布包及显式本地补充重新生成全部既有命令，97 份输出与升级前逐字节一致，没有丢失原有说明、主机或路径补全，也没有扩大命令集合。两个体积异常大的规范 `aws` 和 `gcloud` 继续排除。要添加命令，请使用其名称运行 `convert.mjs`，检查完整转换结果，并提交生成的 JSON 及必要许可。

[fig]: https://github.com/withfig/autocomplete