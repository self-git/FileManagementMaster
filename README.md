# FileManagementMaster

基于 Vue 3、Tauri 2 和 Rust 的本地文件管理应用。「文件管理」和「书库管理」共用实际目录、分支、版本和空间回收。

## 桌面操作

- **打开目录**。首次打开时选择一个目录直接纳管，也可勾选「在所选目录下新建独立管理目录」。后续可从「更多操作 → 打开目录」更换，或按 `⌘O`／`Ctrl+O`。重新打开应用时恢复上次使用的模块和目录，并按该目录的默认分支规则打开。
- **浏览与读取**。在「文件管理」双击文件夹进入下一级，双击文件交由系统默认应用打开。详情提供实际路径和 Finder 或文件资源管理器入口。「读取来源目录」按相对路径预览新增和更新。「更多操作 → 保存外部改动」保存当前分支的目录变更。
- **分支与移动**。侧边栏的「+」创建空分支，行菜单设置默认分支。有外部改动时先确认保存到原分支，再切换。选中文件或书目后点「移至分支」，在预览里核对目标和文件清单。冲突项需填写保留原后缀的新相对路径。确认后源分支、目标分支和书库状态在同一笔 reftable 事务中更新。
- **书目与追更**。在「书库管理」展开书目可选单个文件，点书目行选中整本。「扫描小说」递归查找当前分支，从文件名识别书名、作者和进度。默认格式为 `.epub`、`.txt`、`.mobi`、`.azw3`，可选 `.pdf`、`.azw`、`.kfx`、`.prc`、`.fb2`、`.fb2.zip`、`.kepub`。「导入更新」从来源目录读取，同书同格式仅出现在一个分支时预选目标，多个分支时由用户选择。
- **全选与批量操作**。文件、书目、扫描和读取预览均可全选当前结果。文件夹和书目会展开为其中匹配的文件，筛选后移除不可见的选择。选中多项可一次移至分支，或在读取预览批量设置目标分支和处理方式。需要确认的书目会在提交前标出。
- **版本与空间**。选中文件或书目后打开「查看版本」，书目可按格式查看。各分支引用的当前版另计，普通文件按文件、小说按书目和格式各可找回最近 10 个旧版。恢复前需确认写入位置。待回收对象由后台处理，可在「更多操作 → 空间管理」查看结果或手动回收。
- **键盘与诊断**。`⌘F`／`Ctrl+F` 筛选当前列表，Shift 连选，Command／Ctrl 加选，Esc 取消确认面板。「更多操作 → 诊断」运行随包 Git 自检，Mac 也可从系统「帮助」菜单打开。

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

## 批量操作验收

浏览器回归覆盖全选范围、批量移动、目标分支、处理方式、待确认书目及 420 像素宽窗口。19 项 Rust 测试通过。更新后的 Mac DMG 通过校验、只读挂载和应用签名检查，使用安装副本内的随包 Git 通过了 4 项小说导入回归。

当前 `FileManagementMaster_0.1.0_aarch64.dmg` 的 SHA-256 为 `dcb920c10e55e897d01ab67e512d2a6695063a4db9bb0d1445178a1a33e4bff4`。

| 文件 | 批量操作改动 |
| --- | --- |
| `src/App.vue` | 全选入口、按实际文件预览批量移动及填入建议名称。 |
| `src/LibraryBrowser.vue` | 文件夹和书目的选择范围、筛选联动及列表快捷键。 |
| `src/LibraryReading.vue` | 格式、候选及替换项全选，批量设置及提交校验。 |
| `src/library.css` | 部分选中状态和批量设置布局。 |
| `src-tauri/src/library/reading.rs` | 返回同书同格式的覆盖关系，合并同一文件的移出记录。 |
| `src-tauri/tests/novels.rs` | 验证同批保留新版并归档旧版、副本时的结果。 |
| `README.md` | 操作说明、验收记录和安装包校验值。 |

## 新版桌面验收

macOS 15.6.1 arm64 上验证了新版 DMG、安装副本签名及随包 Git 2.56.1。正式镜像中的 `.app` 与回归用安装副本逐文件一致。原生界面使用同源码、独立应用标识的验收副本和临时书库。交付镜像使用 ad-hoc 签名，Apple 公证待完成。

| 检查 | 结果 |
| --- | --- |
| 两个模块和实际目录 | 从安装版直接纳管中文及空格路径、创建独立子库、读取普通文件。切分支前确认保存外部改动，并在 Finder 核对同一目录中的文件变化。 |
| 跨分支移动和中断 | 整本及单文件移动通过。大小写等价路径被拦截，改名后用随包 Git 核对源分支、目标分支和状态引用。重开时验证了提交前还原原件和提交后修复索引的提示及实际结果。 |
| 扫描、追更和历史 | 原生界面验证递归扫描、同书 TXT 去重时保留 EPUB、跨分支追更及 TXT 旧版恢复。浏览器回归另覆盖多分支目标选择、同格式多个当前文件的恢复预览、键盘、深色和窄窗口。 |
| 版本空间及完整性 | 安装副本的 19 项 Rust 测试全部通过，覆盖每格式旧版窗口、分支当前版保护、超限对象删除和 `git fsck --full --strict`。回收样本的对象库占用由 2,369,154 字节降到 1,954,210 字节。 |
| 长路径与大文件 | 安装副本回归使用 402 字节的相对路径和两份 128 MiB 的文件，覆盖建库、移动、追更、恢复及回收。 |
| Windows x64 | 前端模拟检查通过 Ctrl 快捷键、路径显示和文件资源管理器入口。NSIS 及原生验收待可用的 Windows x64 环境。 |

大文件样本的建库、移出、追更、切换及校验、恢复及校验、回收分别耗时 9.28、4.40、10.92、2.53、5.25、27.27 秒。书库及 `.git` 的文件占用块采样峰值为 672,960,512 字节。

桌面改版验收时的 `FileManagementMaster_0.1.0_aarch64.dmg` 的 SHA-256 为 `00d98dd88e8c22d0d7e4c0977dc70d508c9f9c9135f14e558edcdffe17bb030e`。

### 本轮变更文件

| 文件 | 改动 |
| --- | --- |
| `src/App.vue` | 共用窗口、目录和分支状态，接入移动、历史和回收面板。 |
| `src/LibraryBrowser.vue` | 文件夹和书目列表、格式、进度、搜索与键盘选择。 |
| `src/LibraryReading.vue` | 普通读取、扫描及追更的来源与候选预览。 |
| `src/DesktopSheet.vue` | 窗口内确认面板、键盘焦点与取消。 |
| `src/DesktopIcon.vue` | 工具栏、列表及操作图标。 |
| `src/EngineVerification.vue` | 诊断面板和随包 Git 反馈。 |
| `src/library-types.ts` | 库视图、书目元数据和格式类型。 |
| `src/style.css` | 系统字体、浅深色及共享控件。 |
| `src/library.css` | 双模块布局、窄窗口和预览状态。 |
| `src-tauri/src/lib.rs` | 注册原生文件打开及 macOS 帮助菜单事件。 |
| `src-tauri/src/commands.rs` | 系统打开和定位库内文件及目录。 |
| `src-tauri/src/library.rs` | 库视图元数据、库内实际路径校验与恢复结果。 |
| `src-tauri/src/worktree.rs` | 向上层返回已有中断恢复的结果。 |
| `src-tauri/src/library/movement/tests.rs` | 核对提交前和提交后两种恢复返回值。 |
| `src-tauri/tests/library.rs` | 核对元数据、当前分支路径和链接拒绝。 |
| `README.md` | 界面操作、安装验收及本轮变更。 |
| `技术方案与开发计划.md` | 补记 Mac 改版验收和 Windows 验收状态。 |

## 改版前 Mac 安装验收

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
