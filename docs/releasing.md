# semantic-compact 发版约定

本文件是本项目发版流程的统一入口。用户要求发版、release 或打版本 tag 时先读本文件，并核对实际 Git 与 GitHub 状态。用户最新指令优先。

## 根据 diff 自行选择版本

查看工作区改动、待发布提交，以及最近一次完整 GitHub Release 的 tag 到待发布代码的 `git diff`；不要只看最后一个 commit，也不要只看版本文件。版本选择由代理完成，说明一句理由即可，无需让用户挑版本号。

| 改动 | 版本规则 | 示例 |
| --- | --- | --- |
| 兼容的错误修复、阈值校准、性能优化、诊断改进 | patch：`0.2.9 → 0.2.10` | 修复 transcript 读取、调整已有判断的阈值 |
| 新增兼容能力、配置或判断路径 | minor：`0.2.9 → 0.3.0` | 新增“任务延续但旧上下文过时”的 compact 提醒 |
| 稳定版中破坏既有配置、接口或使用方式 | major：`1.4.2 → 2.0.0` | 删除配置字段且不兼容旧配置 |
| 当前 `0.x` 阶段的破坏性变化 | 至少升 minor，并明确迁移影响 | `0.2.9 → 0.3.0`；不能藏在 patch 中 |
| 仅文档、测试、内部重构，用户行为不变 | 默认不发版本 | 用户明确要求 tag 时可升 patch |

- 多种变化混合时取最高级别；代码量、文件数和重构规模不决定 major。
- `1.0.0` 表示明确建立稳定兼容承诺，不因“大改了很多代码”自动升级。
- 选定版本必须高于已有正式版本和已占用的版本 tag，不能覆盖、移动或删除已推送 tag。
- tag 格式为 `vX.Y.Z`；`Cargo.toml`、`Cargo.lock` 中本包版本、`.claude-plugin/plugin.json` 保持一致。

## 完整发版流程

1. 检查 `git status`、当前分支、`origin`、远端 `main`、tags 和 GitHub Releases。当前仓库为 `https://github.com/LcpMarvel/semantic-compact`；若实际 remote 不同，先查明原因。保留其他在途改动，不强推。
2. 阅读待发布 diff，选择版本并说明依据。确认发布内容只包含相关源码、文档和配置；不提交 `.env`、凭据、真实会话日志、`data/`、本机 `bin/` 或临时产物。
3. 完成必要验证：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`sh scripts/test-setup.sh`、`git diff --check`。相同输入上已通过的检查可复用；判断问题或阈值变化需用无私人信息的代表性会话校准，并说明样本限制。构建目录优先放内部 SSD，例如 `/private/tmp/sc-cargo-target`。
4. 使用用户现有 Git author/committer 提交功能改动并推送 `main`。不添加 AI、模型、工具署名或 `Co-authored-by` 尾注；不重写已有的人类提交。
5. **先读 `.github/workflows/release.yml`，按它实际支持的方式发布。当前工作流仅支持 `workflow_dispatch`，会自行同步版本、创建发布提交及 tag、构建并上传安装包。不要先手工创建同一个 tag。** 当前命令：

   ```sh
   gh workflow run release.yml --repo LcpMarvel/semantic-compact --ref main -f version=X.Y.Z
   ```

   `X.Y.Z` 替换为已选版本。补发已有 tag（tag 已推送但 Release 缺失）改用 `-f from_tag=vX.Y.Z`，工作流会跳过版本同步与打 tag，直接从该 tag 构建发布。两个输入互斥。提交后的 CI 与 release 都要检查；派发成功只表示开始执行，不代表发布成功。该现有工作流的发布提交由 `release-bot` 创建，功能提交仍保留用户署名。
6. 等待对应 release run 完成，确认 tag 指向的代码包含预期改动，并确认 GitHub Release 有工作流矩阵中的四个平台二进制及 `SHA256SUMS`。任一平台构建或上传失败都应报告为“发布未完成”，不能只凭 tag 宣称已发版。
7. 获取远端发布提交和 tag；工作区干净且可快进时同步本地 `main`。需要升级本机插件时使用正式版本，并核对实际安装版本和 hook；不要把替换旧缓存二进制当成正式升级。
8. 回报提交、tag、Release 链接、验证结果及未完成项。在本文件末尾补一条简短发版记录，不复制运行日志。

## 只打 tag、失败重试与本地补丁

- 用户明确只要求 commit / push / tag 时，按该范围执行：本地同步三处版本、验证、提交、创建带说明的 tag，再以 `git push --atomic origin main refs/tags/vX.Y.Z` 推送。明确说明尚未生成 Release 安装包。
- **当前工作流不会因 tag push 自动运行。** tag 已存在而 Release/安装包缺失时，用 `from_tag` 从该 tag 补发（见上文第 5 步）；工作流要求 tag 所指提交的 `Cargo.toml` 与 `plugin.json` 版本和 tag 一致，否则拒绝。优先续跑已存在的失败 run。不要移动旧 tag，也不要把别的提交编译成旧 tag 的安装包。
- 本地缓存补丁只是临时修复，升级可能覆盖。正式版本发布后核对实际安装状态；保留必要备份，不将补丁伪装成已发布版本。
- 若实际使用 Claude 执行器，使用 `zclaude`。

## 发版记录

- 2026-09-27：`v0.2.9`，提交 `dfec56d205afd27fc15dde049b207e794b632278`，已推送到 `main` 和 tag。新增渐进话题漂移提醒与 transcript 错误分类；41 项测试、Clippy、格式检查、安装脚本检查通过，5 组模拟会话实际 API 验证通过。当次仅推送 tag，未生成 GitHub Release 安装包。本机仍是标记为 `0.2.7` 的缓存加本地二进制补丁，备份在 `data/local-patch-20260927/`。按本文件新规则，这类新增判断能力今后应升 minor；已存在的 `v0.2.9` 保持不变。以上为当时状态，下次发版先查远端和本机实际状态。
