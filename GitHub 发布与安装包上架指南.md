# FileManagementMaster GitHub 发布与安装包上架指南

适用对象为项目维护者。当前项目版本为 `0.1.0`，以下用首次发布 `v0.1.0` 演示。

| 修订日期 | 内容 |
| --- | --- |
| 2026-11-22 | 首次上传源码、生成各平台安装包、创建 Release 和后续发版 |
| 2026-11-22 | 新增一键打包和 ChatGPT 网页版交接步骤 |

要把项目交给网页版 ChatGPT 或 Codex 时，先运行 `npm run pack:source` 生成源码 ZIP。[一键打包与交接](#23-一键打包源码与网页版交接)

## 目录

1. [确定首发范围](#1-确定首发范围)
2. [首次上传源码](#2-首次上传源码)
3. [打标签并取得安装包](#3-打标签并取得安装包)
4. [创建并发布 Release](#4-创建并发布-release)
5. [后续发版与常见问题](#5-后续发版与常见问题)

## 1 确定首发范围

同一个版本建一条 Release。例如，`v0.1.0` 的 **Assets** 可以同时放 Mac DMG 和 Windows 安装程序，下载者按自己的平台选择。仓库设为 **Private** 时，给需要下载的人授予仓库访问权限。设为 **Public** 时，源码和已发布的安装包都可公开访问。[GitHub Releases 说明](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)

| 平台 | 发布文件 | 当前状态 |
| --- | --- | --- |
| Mac Apple Silicon（arm64） | `FileManagementMaster_0.1.0_aarch64.dmg` | 本机产物已通过安装验收。内含应用使用 ad-hoc 签名，Apple 公证待完成。 |
| Windows x64 | NSIS 生成的 `*-setup.exe` | 已配置构建和安装回归。本机暂无成品，Windows 原生安装验收待执行。 |
| Mac Intel（x86_64） | 单独构建的 Intel DMG | 当前工作流没有此构建任务。需要该平台时，在 Intel Mac 或 `macos-15-intel` 上构建同架构的应用和随包 Git，并完成安装验收。 |

本机已验收的 DMG 位于 `src-tauri/target/release/bundle/dmg/FileManagementMaster_0.1.0_aarch64.dmg`，校验值在项目验收记录中。当前源码还有打包后新增的界面文件。发布本次导入的源码时，按第 3 节从发布标签重新构建。[本项目 Mac 验收记录](README.md#五期-mac-安装验收)

首发可以先让有限的测试者试用 Apple Silicon 版，在 Release 中标记预发布。公开分发 Mac 版前，按 Tauri 的流程用 **Developer ID Application** 签名并完成 Apple 公证，在另一台 Mac 上通过网页下载验证。当前 Actions 的 Mac 构建使用 ad-hoc 签名，公开分发时要使用实际签名身份和公证配置。[Tauri macOS 签名](https://v2.tauri.app/distribute/sign/macos/) · [Apple 公证文档](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

向他人分发内含 Git 的安装包时，同时准备许可证和完整对应源码。当前 Mac 包已含 Git 的 `COPYING`，其对应源码可使用清单锁定的 `git-2.56.1.tar.xz`。将源码压缩包和对应构建脚本随该版本提供。发布 Windows 包前，按实际随包 MinGit 版本核对各组件的分发材料。项目根目录当前没有 `LICENSE`，如计划开放项目源码的使用与分发权限，先明确项目自身的许可。[Git 运行时清单](scripts/git-runtime.json) · [Git 构建脚本](scripts/prepare-git.mjs) · [Git 2.56.1 许可第 3 条](https://github.com/git/git/blob/v2.56.1/COPYING) · [GitHub 仓库许可说明](https://docs.github.com/en/repositories/creating-and-managing-repositories/licensing-a-repository)

## 2 首次上传源码

### 2.1 在 GitHub 创建空仓库

打开 GitHub 的 **New repository**，选择仓库所有者，名称可用 `FileManagementMaster`。按第 1 节选择可见性，保持 README、.gitignore 和许可证初始化选项关闭。创建后，从 **Quick setup** 复制 HTTPS 仓库地址。[新建仓库](https://github.com/new) · [GitHub 导入本地源码说明](https://docs.github.com/en/migrations/importing-source-code/using-the-command-line-to-import-source-code/adding-locally-hosted-code-to-github)

### 2.2 在 Mac 终端提交并推送

当前项目目录尚未初始化 Git。在终端进入项目根目录。如果用的是源码包，进入解压出的 `FileManagementMaster/`。运行下面的命令后先检查暂存清单。本项目的安装包、依赖目录和构建缓存由现有 `.gitignore` 排除，日志目录也留在提交范围之外。检查清单时排除个人书库、真实样本、账号凭据和签名私钥。

```bash
git init -b main
git add -- .gitignore .github scripts src src-tauri index.html \
  package.json package-lock.json tsconfig.json vite.config.ts README.md \
  '需求说明.md' '技术方案与开发计划.md' 'GitHub 发布与安装包上架指南.md'
git status --short
git diff --cached --name-only
git diff --cached --check
```

确认清单后再提交。如果 Git 提示缺少作者信息，用 `git config --local user.name "你的名字"` 和 `git config --local user.email "你的提交邮箱"` 为此仓库设置身份。下面的仓库地址换成刚才复制的实际地址。在浏览器完成 GitHub CLI 授权时，`workflow` 权限用于推送本仓库的 Actions 工作流文件。[GitHub CLI 登录说明](https://cli.github.com/manual/gh_auth_login) · [GitHub OAuth 权限说明](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps)

```bash
git commit -m "Initial import of FileManagementMaster"
gh auth login --hostname github.com --git-protocol https --web --scopes workflow
gh auth setup-git --hostname github.com
git remote add origin 'https://github.com/YOUR-ACCOUNT/FileManagementMaster.git'
git remote -v
git push -u origin main
```

完成后，GitHub 仓库的 **Code** 页应显示 `main`、源码和 `.github/workflows/desktop.yml`。如果已有登录且推送提示工作流权限不足，可用 `gh auth refresh --hostname github.com --scopes workflow` 补充授权后重试。[GitHub CLI 更新授权](https://cli.github.com/manual/gh_auth_refresh)

### 2.3 一键打包源码与网页版交接

在 Mac 终端进入项目根目录，运行以下命令。成功后终端会打印 `exports/FileManagementMaster-source-*.zip` 的完整路径，ZIP 内的顶层目录为 `FileManagementMaster/`。[打包脚本](scripts/pack-source.sh)

```bash
npm run pack:source
```

如果要让另一个 AI 在能访问项目目录的环境里制作 ZIP，也让它在项目根目录执行这条命令。脚本按以下范围取当前工作目录中的文件。

| 收录内容 | 位置 |
| --- | --- |
| Vue 页面和样式、Rust 源码和回归测试 | `src/`、`src-tauri/src/`、`src-tauri/tests/` |
| 构建入口、依赖锁文件、Tauri 配置、权限和图标 | 根目录的前端清单与配置、`src-tauri/` 的清单与配置、`src-tauri/capabilities/`、`src-tauri/icons/` |
| 安装包工作流、随包 Git 的构建脚本、项目资料 | `.github/workflows/desktop.yml`、`.gitignore`、`scripts/` 的打包脚本和运行时清单、根目录项目文档 |

ZIP 排除真实书库、日志、`.git`、`node_modules`、`dist`、`src-tauri/target`、`src-tauri/gen`、已生成的随包 Git 和安装包。安装包在目标平台按标签源码重新构建。

普通 ChatGPT 的 GitHub 连接提供只读访问。支持数据分析的会话可以尝试打开 ZIP。需要把源码提交到 GitHub 时，使用已获目标仓库写权限的 Codex 网页端或其他能写入仓库的工具，并确认该会话能读取 ZIP 内容。[ChatGPT GitHub 连接说明](https://help.openai.com/en/articles/11145903-connecting-github-to-chatgpt) · [ChatGPT 文件上传说明](https://help.openai.com/en/articles/8555545-uploading-files-and-audio-to-chatgpt) · [Codex Cloud](https://learn.chatgpt.com/docs/cloud)

上传 ZIP 后，把下面提示词里的仓库地址换成实际地址。

```text
请检查你能否解压附件，以及是否有目标 GitHub 仓库的写入权限。
目标仓库为 https://github.com/YOUR-ACCOUNT/FileManagementMaster。
如果具备权限，请把 FileManagementMaster/ 里的内容放到仓库根目录，
保留 .gitignore、.github/workflows/desktop.yml、两个依赖锁文件、Tauri 配置和图标。
现有仓库用独立分支提交并打开 Pull Request。完成后给出提交或 PR 链接。
如果当前只能读取，请告诉我缺哪项能力，并给我能读取的文件清单。
本次先提交源码。安装包通过对应标签的构建和验收后，再按指南创建 Release。
```

如果当前网页会话没有解压能力或仓库写权限，在本机解压进入 `FileManagementMaster/`，按第 2.1、2.2 节上传源码。

## 3 打标签并取得安装包

### 3.1 固定发版提交

确认要发布的改动已提交并推送，`package.json`、`package-lock.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 和 `src-tauri/Cargo.lock` 中本项目版本均为 `0.1.0`。在项目目录的 `main` 上创建并推送标签。第一条命令应无输出，第二条应显示准备发版的提交。

```bash
git status --short --untracked-files=no
git log -1 --oneline
git tag -a v0.1.0 -m "FileManagementMaster v0.1.0"
git push origin v0.1.0
git rev-parse 'v0.1.0^{commit}'
```

### 3.2 在 Actions 下载通过验收的产物

1. 在仓库点 **Actions → Desktop installer acceptance**，打开这次标签推送触发的运行。用 `v0.1.0` 和提交 SHA 确认构建对应第 3.1 节的源码。
2. 查看需要发布的平台任务。安装包在安装回归之前就上传到了 Actions，因此要确认**对应平台任务最终成功**，再下载该任务的产物。Windows 还需在目标系统安装并启动应用做一次原生验收。
3. 在该次运行的 **Summary → Artifacts** 下载下面的产物并解压。上架时选解压出的安装文件。

| Actions 产物 | 平台 | 解压后用于上架的文件 |
| --- | --- | --- |
| `FileManagementMaster-macos-15` | Apple Silicon | `FileManagementMaster_0.1.0_aarch64.dmg` |
| `FileManagementMaster-windows-2025` | Windows x64 | 实际生成的 `*-setup.exe` |

工作流完成后，把需要上架的安装文件从 **Artifacts** 取出，上传到对应 Release 的 **Assets**。[本项目桌面工作流](.github/workflows/desktop.yml) · [GitHub 下载工作流产物](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts) · [GitHub 托管运行器架构](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)

如需本机打包，从同一个发布标签对应的源码目录执行 `npm ci` 和 `npm run git:verify`，再按下表构建。Mac 打包需要 Xcode 命令行工具。Windows x64 打包需要 MSVC C++ 构建工具和 WebView2。

| 构建主机 | 打包命令 | 产物目录 |
| --- | --- | --- |
| Apple Silicon Mac | `npm run desktop:build -- --bundles dmg` | `src-tauri/target/release/bundle/dmg/` |
| Windows x64 | `npm run desktop:build -- --bundles nsis` | `src-tauri/target/release/bundle/nsis/` |

打包后用实际产物做一次安装和启动验收。[Tauri 构建工具要求](https://v2.tauri.app/start/prerequisites/) · [DMG 打包](https://v2.tauri.app/distribute/dmg/) · [Windows NSIS 安装程序](https://v2.tauri.app/distribute/windows-installer/)

## 4 创建并发布 Release

1. 在 GitHub 仓库页点 **Releases → Draft a new release**。
2. 在 **Choose a tag** 选择已经推送的 `v0.1.0`。标题可用 `FileManagementMaster v0.1.0`。说明里列出这次支持的系统和架构，以及包含的主要改动。
3. 将验收通过的 `.dmg` 和 `*-setup.exe` 拖到附件区。如果这次只有 Mac 通过验收，草稿只附 Mac 安装包。随包 Git 源码等材料也放在附件区，并在发布说明中单独注明用途。
4. 首次试用版勾选 **This is a pre-release**，点击 **Save draft**。核对标签、附件文件名和平台说明后，再点 **Publish release**。
5. 发布后打开 `https://github.com/YOUR-ACCOUNT/FileManagementMaster/releases/tag/v0.1.0`，把 `YOUR-ACCOUNT` 换成实际所有者。用目标受众的访问权限打开页面，从 **Assets** 下载并验证安装。

GitHub 会同时显示 **Source code (zip)** 和 **Source code (tar.gz)**，它们对应这个标签的源码。给用户分享 Release 页面，并指明 **Assets** 中适合其平台的安装文件。[GitHub 创建 Release](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository) · [GitHub 自动生成的源码归档](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)

如果首发只发 Mac 测试版，发布说明可直接用这一版。

```markdown
## 安装

- Apple Silicon Mac（arm64）下载 FileManagementMaster_0.1.0_aarch64.dmg。
- 本版用于试用，应用使用 ad-hoc 签名，尚未完成 Apple 公证。

## 本版功能

支持本地书库、分支切换和跨分支移书、小说扫描去重、追更导入与旧版恢复。
```

## 5 后续发版与常见问题

### 5.1 发下一版

将项目版本统一更新到 `0.1.1`。可以用 `npm version 0.1.1 --no-git-tag-version` 同步前端清单与锁文件，再更新 Tauri 配置及 Cargo 包版本。运行 `cargo check --manifest-path src-tauri/Cargo.toml` 后，检查 `Cargo.lock` 中 `filemanagement-master` 的版本。提交并推送这些改动，然后用 `v0.1.1` 重复第 3、4 节。从新标签收集产物，并保留旧 Release 供查询。[npm version 命令](https://docs.npmjs.com/cli/v11/commands/npm-version)

### 5.2 为什么 Actions 里有安装包，Releases 里还没有？

Actions 的 **Artifacts** 属于某一次构建。将解压出的安装文件上传到对应的 Release 草稿并发布后，下载者才能从 **Releases → Assets** 获取安装包。[GitHub 下载工作流产物](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts) · [GitHub 创建 Release](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)

### 5.3 找不到 Windows 安装包怎么办？

查看本次标签下的 `windows-2025` 任务。若任务失败，打开失败步骤的日志处理后再运行。确认实际生成的 NSIS 安装包通过验证后，再把它加入发布草稿。

### 5.4 下载的 Mac 安装包无法打开怎么办？

先确认安装包架构与本机芯片匹配，并查看该版本的签名和公证状态。公开分发版应完成 Developer ID 签名与 Apple 公证，并从 Release 重新下载后复验。[Apple 公证文档](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
