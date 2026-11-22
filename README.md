# FileManagementMaster

基于 Vue 3、Tauri 2 和 Rust 的本地桌面书库。支持同库分支、小说扫描去重、手动读取、追更导入、版本恢复和旧版空间回收。

## 书库操作

- 选中目录后，默认直接纳管其中的文件。勾选「在所选目录下创建独立书库」可新建一个实际子目录和独立 Git 仓库。
- 建库时创建「常看」分支。通过「新分支」可以创建空分支，通过「设为默认」指定下次打开的分支。
- 切换前列出外部新增、修改、删除及可识别的原内容改名。确认后先保存到原分支，再切换。同一目录中的实际文件会随分支变化。启动时打开默认分支也会读取并确认尚未保存的外部改动。
- 勾选同一本书的各格式和分卷，填写书目名并创建书目。可以点「选择整本」移动该书在当前分支的全部文件，也可以勾选单个文件，选定目标分支后预览移动。发现外部改动时，先确认保存至原分支，再预览移动。
- 目标同路径的文件内容不同或目录路径冲突时，为每项填写保留原后缀的新相对路径。确认后，源分支、目标分支和书库状态在一笔 reftable 事务中发布。当前分支继续显示源目录，切到目标分支即可查看移入文件。
- 点「扫描与读取」可递归扫描当前分支，或从来源目录选择文件。小说默认搜索 .epub、.txt、.mobi、.azw3，另可勾选 .pdf、.azw、.kfx、.prc、.fb2、.fb2.zip、.kepub。扫描支持大小写后缀和多层目录，识别时只使用文件名。
- 小说预览显示书目、格式和正文进度。正文覆盖明确时可勾选替换同格式旧文件，缺作者、别名和覆盖不明的版本可在预览中选择。追更只有一个分支有同书同格式时预选该分支，有多个时需指定目标。普通文件可按相对路径读取和更新。
- 选中文件后点「查看版本」可查看各分支使用的当前版及可恢复的旧版。小说按书目和格式、普通文件按文件各保留最近 10 个旧版。恢复旧版会确认写入路径并保存一次新版本。书库写入后后台清理过期对象，也可点「回收空间」处理待回收任务。
- 点按「打开实际文件夹」可以在 Finder 或 Windows 文件资源管理器中看到当前目录。「Git 引擎验证」在临时库中检查随包 Git、引用事务和对象回收。

## 开发

使用 Node.js 22.12 或更高版本、Rust 1.90 或更高版本及目标平台的 Tauri 工具链。本机验收使用 Rust 1.96。macOS 需要 Xcode 命令行工具，Windows 使用 x64、MSVC C++ 构建工具和 WebView2。

```sh
npm ci
npm run desktop:dev
```

首次运行会按 [Git 运行时清单](scripts/git-runtime.json) 下载并校验归档。macOS 在本机构建 Git 2.56.1，Windows 解包固定版 MinGit。把环境变量 `FILEMANAGEMENT_GIT_ARCHIVE` 设为已下载归档的绝对路径，可使用同一版本的本地归档，仍会校验 SHA-256。

## 验证和打包

```sh
npm run git:verify
npm run desktop:build
```

`git:verify` 覆盖 Git 引擎、建库与切分支、书目与单文件移动、同名冲突、旧引用校验及中断恢复。设置 `FILEMANAGEMENT_BUNDLED_GIT_DIR` 可以对安装包里的运行时执行相同测试。macOS 对应 `.app/Contents/Resources/git`，Windows 对应应用安装目录下的 `git`。

macOS 安装包可用 `npm run desktop:build -- --bundles dmg` 构建，产物在 `src-tauri/target/release/bundle/dmg/`。Windows 原生构建生成 NSIS 安装包。[桌面验证流程](.github/workflows/desktop.yml) 保留安装包，并从安装目录运行回归测试。

macOS arm64 已在原生应用窗口和应用包内运行时通过验收。Windows x64 按固定版 MinGit 的 `ucrt64` 布局打包，原生安装和运行由 Windows 构建任务验收。

Mac 的书库验收使用临时目录。在应用重启时确认保存外部改动，再切换到默认分支，并在 Finder 的同一目录观察文件移出和切回后的恢复。独立子书库也已确认拥有自己的 reftable 仓库。

第四阶段的版本树保护所有分支的当前版。在每个保留单位内，对仍无分支引用的内容按保存顺序保留最近 10 个不同旧版。实际回收通过随包 Git 并持有书库写锁。Windows 安装产物的 Git 和回收进程由现有原生 CI 验证。

第三阶段在 Mac 原生应用内移入了一本书的 EPUB 和两个 TXT 分卷，再将一个同名异内容的 TXT 改名移入。Finder 在同一路径显示了源分支移出与目标分支加入后的文件。使用应用包内 Git 核对两次移动的三个引用、提交树和原文件字节，并运行了部分暂存、准备阶段强制终止 Git、引用发布后未更新索引的恢复测试。Windows 原生 CI 纳入了文件占用导致移动整体回退的测试，当前验收环境是 Mac。

## 五期 Mac 安装验收

验收使用 macOS 15.6.1、Apple Silicon 和随包 Git 2.56.1。正式版 `FileManagementMaster_0.1.0_aarch64.dmg` 内的应用使用 ad-hoc 签名，Apple 公证待完成。磁盘映像的校验和、安装副本及随包 Git 的签名检查均通过。回归从正式 DMG 的安装副本执行，原生界面使用同一代码构建的独立标识 DMG，书库与首选项分别存放。

| 场景 | 实测结果 |
| --- | --- |
| 中文和空格路径 | 在安装版创建独立子书库、直接纳管已有目录、切分支、移入 TXT 与 EPUB、追更、恢复及回收。 |
| 大小写冲突 | `Cover.txt` 与目标 `cover.TXT` 冲突。等价改名仍被拒绝，三个 Git 引用保持原值。改用新文件名后，两份原文均保留。 |
| 长路径和大文件 | 正式安装副本使用 362 字符（402 字节）的相对路径及两份各 128 MiB 的样本。建库、移出、追更、切换和恢复后以随包 Git 核对内容。 |
| 强制退出与重启 | 128 MiB 原件已暂存、三个引用仍为旧值时强制退出。重启后原件的 SHA-256、三个引用与中断前一致，索引已对齐。 |
| 回收及完整性 | 正式安装副本通过 18 项 Rust 回归。版本窗口样本的对象库从 2,369,096 字节降至 1,954,250 字节，`git fsck --full --strict` 通过。 |

大文件样本的建库、移出、追更、切换与校验、恢复与校验、回收分别耗时约 4.26、1.84、4.81、1.26、3.17、12.72 秒。对书库及其 `.git` 每 20 毫秒采样，文件占用块峰值为 689,131,520 字节，约 657 MiB。

交付 DMG 的 SHA-256 为 `0cff1f6ab2c95d9e1489bf56085198685a4b60f568f874c6cb3aeeda1411ea71`。

此阶段的改动涉及 `.github/workflows/desktop.yml` 的 DMG 安装与验收流程、`src-tauri/tests/release.rs` 的长路径和大文件回归，以及本页的发行记录。

## 四期文件

Mac 原生应用已验证默认与可选后缀、递归扫描、TXT 去重时保留 EPUB、普通文件读取、追更导入到非当前分支及旧版恢复。对象库测试为 TXT 和 EPUB 各准备 15 个版本，核对另一个分支的当前版以及跨保留单位共用的对象都仍可读取。过期对象通过 cat-file 验证已移除，并检查对象库大小及 fsck 结果。

| 文件 | 变更 |
| --- | --- |
| src/App.vue、src/LibraryReading.vue | 扫描、读取预览、历史恢复和后台回收 |
| src/library-types.ts、src/library.css | 共享界面类型和扫描及历史界面样式 |
| src-tauri/src/library.rs | 书目作者、版本原路径、严格保留窗口与分支发布 |
| src-tauri/src/library/novel.rs | 后缀、书名、作者、卷章、版本系列和副本识别 |
| src-tauri/src/library/reading.rs | 递归扫描、书目候选、去重预览及追更导入 |
| src-tauri/src/library/history.rs | 按保留单位查询和恢复版本，测量对象回收 |
| src-tauri/src/library/movement.rs、src-tauri/src/library/movement/tests.rs | 复用目标路径及稳定书目 ID，更新移动历史验证 |
| src-tauri/src/worktree.rs | 同分支写入暂存及排他回收记录 |
| src-tauri/src/git.rs | 回收进程启动凭证、随包 Git 执行和对象库字节统计 |
| src-tauri/src/commands.rs、src-tauri/src/lib.rs、src-tauri/src/main.rs | 注册桌面命令和应用包回收进程入口 |
| src-tauri/Cargo.toml、src-tauri/Cargo.lock | 使用既有锁定版本的正则解析依赖 |
| src-tauri/tests/novels.rs、.github/workflows/desktop.yml | 新增导入、恢复及物理回收验收，在双端安装产物执行 |
| README.md | 第四阶段操作说明、验证与改动清单 |

## 三期文件

| 文件 | 变更 |
| --- | --- |
| `src/App.vue` | 书目归组、整本或文件选择、外部改动与移动确认 |
| `src/library.css` | 书目、选中文件和冲突输入样式 |
| `src-tauri/src/library.rs` | 持久化书目归属并提供当前分支书目视图 |
| `src-tauri/src/library/movement.rs` | 预览、目标路径校验、保留文件身份和三引用发布 |
| `src-tauri/src/library/movement/tests.rs` | 移动、冲突、中断、历史连续性和 Windows 文件占用测试 |
| `src-tauri/src/worktree.rs` | 源文件暂存、重开回退或完成，以及原件核对 |
| `src-tauri/src/git.rs` | Git 写进程退出检查和实际索引原子更新 |
| `src-tauri/src/verification.rs` | 将引擎诊断的操作记录接入相同数据结构 |
| `src-tauri/src/commands.rs` | 面向原生窗口的创建书目和移动命令 |
| `src-tauri/src/lib.rs` | 对外导出移动类型并注册命令 |
| `src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` | 使用已锁定的 ICU 规范化组件识别等价路径 |
| `README.md` | 第三阶段操作、验收与改动说明 |

## 二期文件

| 文件 | 变更 |
| --- | --- |
| `src/App.vue`、`src/library.css` | 书库、分支和外部改动确认界面 |
| `src/EngineVerification.vue` | 保留从书库窗口可打开的一期引擎验证 |
| `src-tauri/src/library.rs` | 建库、目录状态、稳定文件 ID、版本窗口及默认分支 |
| `src-tauri/src/git.rs`、`src-tauri/src/worktree.rs` | Git 对象流式读写、工作目录暂存、引用提交及操作恢复 |
| `src-tauri/src/commands.rs`、`src-tauri/src/lib.rs` | 系统目录选择、文件管理器、启动记忆和书库命令 |
| `src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` | 固定 Tauri 原生插件及 Windows 文件移动接口依赖 |
| `src-tauri/src/verification.rs`、`src-tauri/tests/library.rs` | 一期验证结构适配和二期书库回归 |
| `package.json`、`.github/workflows/desktop.yml` | 让双端构建验证执行全套 Rust 测试 |
| `README.md` | 第二阶段运行与文件说明 |

## 一期文件

| 文件 | 职责 |
| --- | --- |
| `package.json`、`package-lock.json` | 前端依赖、锁定版本和开发、测试、打包入口 |
| `index.html`、`src/main.ts` | 应用页面入口、挂载 Vue 并加载样式 |
| `src/App.vue`、`src/style.css` | 原生 Git 验证窗口、命令类型及界面样式 |
| `tsconfig.json`、`vite.config.ts` | TypeScript 检查及 Vite 构建配置 |
| `scripts/git-runtime.json`、`scripts/prepare-git.mjs` | 固定 Git 来源和校验值，准备本机随包运行时 |
| `src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/build.rs` | Rust 包及依赖锁定，接入 Tauri 构建 |
| `src-tauri/src/main.rs`、`src-tauri/src/lib.rs`、`src-tauri/src/commands.rs` | 启动应用、注册命令、从应用资源目录运行验收 |
| `src-tauri/src/git.rs` | 固定版 CLI、临时索引、引用事务、Git 子进程和回收 |
| `src-tauri/src/worktree.rs` | 书库排他写锁、操作记录原型和已退出子进程的锁清理 |
| `src-tauri/src/verification.rs`、`src-tauri/tests/git_engine.rs` | 实际临时仓库的验证链路和集成测试入口 |
| `src-tauri/tauri.conf.json`、`src-tauri/tauri.macos.conf.json`、`src-tauri/tauri.windows.conf.json` | 桌面应用及随包资源配置、Mac 与 Windows 打包目标 |
| `src-tauri/capabilities/default.json` | 主窗口使用的 Tauri 权限 |
| `src-tauri/icons/icon.png`、`src-tauri/icons/icon.icns`、`src-tauri/icons/icon.ico` | 通用窗口、macOS 包和 Windows 包图标 |
| `.github/workflows/desktop.yml` | 两端原生构建和从安装产物验收 Git |
| `.gitignore` | 忽略下载、编译和打包产物 |
| `README.md` | 项目运行说明和本表 |
