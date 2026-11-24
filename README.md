# TianShell (v0.0.1)

一个跨平台的 SSH/SFTP 协同工作台，内置标准 **MCP (Model Context Protocol)** 服务。既可以在浏览器里管理多台服务器，也可以让支持 MCP 的助手（TianCode / Cursor / Claude Desktop）连上同一套已保存的主机。

从这一版起，**启动工作台不再需要 Node.js**。`cargo run --bin tianshell` 默认打开原生窗口；`cargo run --bin tianshell -- serve` 只在浏览器里打开。Rust 进程提供页面、HTTP API、终端 WebSocket，并在同一个端口上自动启动 MCP。界面源码在 `src/renderer`，`npm run build` 生成 `ui/` 后由这个进程送出去。

接入 MCP 的完整说明在 [docs/mcp.md](docs/mcp.md)。

作者是 zq（[zhangqi77ok-sys](https://github.com/zhangqi77ok-sys)）。

## 核心特性

- **现代沉浸式桌面工作台 (v0.0.1)**：
  - **Mac 质感无框暗黑顶栏**：半透明毛玻璃质感与 Windows 原生最小化、最大化、关闭控制按钮无缝融合，支持双击缩放与原生平滑拖拽。
  - **Tab 绝对居中锁定 (0 抖动)**：顶部「终端 / 文件 / 监控」视图标签采用绝对居中定位，多视图与控制区切换时保持视觉零跳动。
  - **一键并排分屏 (`Ctrl+\`)**：顶栏常驻 `[◫ 并排分屏]` 按钮，支持终端与 SFTP 左右同屏沉浸式工作流；切到监控大盘再切回自动保持分屏记忆。
  - **左侧主机管理对称设计与双击即连**：等宽紧凑网格（+主机 / +分组 / 隧道）、单行优雅连接胶囊；支持双击卡片直接连接或瞬切会话，悬浮触发编辑与删除动作。
  - **常用开发运维动作库**：集成 12 项高频运维命令卡片（容器查看/日志追踪/清理、端口监听排查、进程资源体检、TCP 链路统计等），一键填入终端。
  - **专属定制「T」字终端徽标**：内嵌 PE 资源与多分辨率 Windows 运行时图标，在窗口与任务栏完美呈现。
- **Rust 工作台，网页界面仍由这个进程提供**：
  - `cargo run --bin tianshell -- serve` 不启动 Node，也不再打包 Electron。
  - 凭据使用 **AES-256-GCM** 加密存储；Windows 下通过系统原生 **DPAPI (`CurrentUser`, `TS1D` 封装)** 保护 `vault.key`，POSIX 下新的钥匙文件是 `TS1R`（仅当前用户可读），旧的 `TS1D` / Node 写出的 `TS1R` 都能读。
  - **高并发与内存防线**：会话与后台执行引擎配备严格的容量淘汰控制（LRU Bounded Eviction），彻底避免长时间长命令自动化下的内存膨胀。
- **主机分组、标签与 ProxyJump 多跳跳板机**：
  - 左侧支持自定义分组、关键词/标签搜索过滤、密码与私钥认证、TOFU 主机密钥指纹记忆与清除。私钥可以直接粘贴日常使用的 OpenSSH 格式（含 ed25519，可以带口令），也可以用 PKCS#8、PEM RSA 或 EC PEM。
  - 主机可配置 `jumpHostId` 跳板机链路（内置循环依赖检测），通过多跳 `forwardOut` 直达内网节点。
- **多标签终端 & 本机 Shell & 广播输入**：
  - 同一个 SSH 标签在界面上仍是一个会话。交互终端独占一条 SSH 连接；SFTP 和监控走同一条不另开标签的第二条连接，所以采样、列目录和大文件传输不会挡住键盘和回显。关掉标签时两条连接一起拆掉。端口转发仍然单独占一条连接，因为它要一直握着转发通道。
  - 同时开启多台 SSH 终端与本机 Shell（Windows PowerShell / POSIX `$SHELL`）标签页，互不关闭。本机终端走系统 PTY（Windows 上是 ConPTY），窗口尺寸会跟着变。
  - **多标签键盘巡航**：支持通过 `Ctrl+PageUp` / `Ctrl+PageDown` 键盘循环快速切标签，焦点自动保持在终端输入区。
  - 终端、文件和监控共用一条会话标签，可以不离开当前视图换会话。`Ctrl+F` 在终端里查找。广播默认关闭；打开后按键进入当前终端，以及标签上另外勾选且仍连接的会话。本机终端和已断开的标签默认不勾选。
- **完整 SFTP 文件管理与在线文本编辑**：
  - **多维极速排序与即时过滤**：表头支持按「名称」、「大小」、「修改时间」点击即时排序（▲/▼ 状态指示，目录置顶），过滤框支持 `Ctrl+F` 内存级模糊搜索，耗时 $<1\text{ms}$，**0 网络开销**。
  - **现代磨砂上下文菜单**：文件与空白处右键菜单支持复制绝对路径/文件名、重命名、修改权限 (`chmod`)、流式下载、打包下载 (`.tar.gz`) 及新建目录。
  - 支持目录浏览、面包屑导航、显示隐藏文件、新建目录、重命名、权限修改 (`chmod`)、递归删除。
  - 支持文件**拖拽上传**、流式下载、底部实时传输队列进度与取消；双击文本/配置文件可弹出等宽编辑器直接修改并写回远端。
- **服务器实时 Bento 监控仪表盘**：
  - 定时低开销采集主机 CPU %、内存 & Swap 占用、网络上下行速率（RX/TX B/s）、LoadAvg、磁盘分区及 Top CPU 进程榜（高负载自动告警变色）。
- **SSH 端口转发与 SOCKS5 动态代理**：
  - 支持 `-L`（本地转发）、`-R`（远程反向转发）及内置 RFC 1928 **-D 动态 SOCKS5 代理**，规则持久化且一键启停。
- **快捷命令库与暗/亮双主题**：
  - 内置常用 Linux 诊断命令卡片并支持自定义分类 CRUD、暗/亮双主题、终端字号与采样周期持久化配置。

## 快捷键速查表 (Keyboard Shortcuts)

| 快捷键 | 作用范围 | 功能说明 |
| :--- | :--- | :--- |
| `Ctrl + \` | 全局 | 一键切换「终端 + SFTP」左右并排分屏 |
| `Ctrl + PageUp` / `Ctrl + PageDown` | 终端视图 | 快速循环切换上一个 / 下一个终端会话标签 |
| `Ctrl + F` | 终端 / SFTP | 终端内呼出全文检索条；SFTP 中聚焦即时过滤搜索框 |
| `Backspace` / `Alt + ↑` | SFTP 视图 | 返回上一级目录 |
| `F5` / `Ctrl + R` | SFTP 视图 | 强制刷新当前目录（清除缓存并拉取最新列表） |
| `Escape` | 全局 | 关闭右键菜单、退出过滤搜索或取消正在编辑的路径 |
| `双击主机条目` | 左侧主机树 | 快速连接未连主机，或瞬切至已打开的该主机标签页 |

## 下载与安装 (Windows)

从 GitHub [Releases](https://github.com/zhangqi77ok-sys/tianshell/releases) 下载最新安装包：
- **`TianShell-Setup-0.0.1.exe`**：标准 Windows 单文件安装包（体积仅约 5.8 MB）。

### 安装与更新特性
1. **自定义安装目录**：支持用户自由选择安装路径，默认安装至 `%LOCALAPPDATA%\Programs\TianShell`（免管理员权限）。
2. **智能更新识别**：当检测到系统已安装旧版本时，安装向导自动读取注册表锁定已有安装目录，默认原位平滑升级。
3. **数据安全隔离**：用户的主机配置、分组标签与加密凭据全部存放于 `%APPDATA%\tianshell`，与程序文件物理隔离，版本更新绝不丢失数据。
4. **标准安全卸载**：可在 Windows「设置 > 应用 > 安装的应用」或控制面板中一键卸载；卸载向导会提示是否保留用户配置数据（默认推荐保留）。

### 开发者打包安装程序

在仓库根目录下执行构建脚本：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-installer.ps1
```

该脚本将自动执行 `cargo build --release` 编译优化二进制，并调用 NSIS 编译生成 `target\TianShell-Setup-0.0.1.exe`。

## 环境与运行 (源码开发)

启动工作台需要 Rust（stable）。已经提交的 `ui/` 可以直接跑，改界面时才需要 Node.js。

```bash
cargo run --bin tianshell
```

进程绑定 `127.0.0.1`，默认端口 `53921`，并打开原生窗口。只要浏览器、不创建窗口：

```bash
cargo run --bin tianshell -- serve
```

加上 `--no-open` 则只打印地址，不打开浏览器。`tianshell --help` 只打印说明。

MCP 随这个进程一起听在 `http://127.0.0.1:53921/mcp`。令牌、端口和给其他客户端的配置见 [docs/mcp.md](docs/mcp.md)。

发布用的可执行文件：

```bash
cargo build --release --bin tianshell
./target/release/tianshell serve
```

生成的是 `target/release/tianshell`（Windows 上是 `tianshell.exe`）。GitHub Actions 的 Windows 任务使用 runner 默认的 MSVC 目标。仓库里不提交 `.cargo/config.toml`，也不提交某台机器的 `linker` / `ar` 绝对路径。

### Windows 安装包

不需要记住 `cargo run` 的参数。在 Windows 上于仓库根目录执行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1
```

脚本用默认工具链执行 `cargo build --release --locked`，打出 `dist\windows\TianShell-windows.zip`。GitHub Actions 的 `windows-package` 任务在 `windows-latest` 上做同一件事，并把 zip 留在构建产物里。

解压后双击 `tianshell.exe` 会打开工作台窗口。控制台要留着，关掉它进程就停了。想进开始菜单：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

这会把程序复制到 `%LOCALAPPDATA%\TianShell`，并放一个「TianShell」快捷方式。快捷方式不带额外参数。

本机如果只用 w64devkit、没有 MSVC，把配置写到用户目录 `%USERPROFILE%\.cargo\config.toml`，路径改成自己的安装位置。默认工具链是 MSVC 时，先执行 `rustup target add x86_64-pc-windows-gnu`。这时可执行文件在 `target/x86_64-pc-windows-gnu/release/`。

```toml
[build]
target = "x86_64-pc-windows-gnu"

[target.x86_64-pc-windows-gnu]
linker = "C:\\path\\to\\w64devkit\\bin\\gcc.exe"
ar = "C:\\path\\to\\w64devkit\\bin\\ar.exe"
```

### 原生窗口

默认构建包含桌面窗口（`desktop` 特性。Windows 用 WebView2，Linux 还要有 WebKitGTK）：

```bash
cargo run --bin tianshell
```

只要浏览器、不要原生窗口时用 `serve`、`--serve` 或 `--no-window`。`--no-open` 不自动打开浏览器，`--port` 改端口。`tianshell --help` 只打印说明并退出，不会把工作台拉起来。

窗口打开的就是上面同一个 Rust 服务。

### 改界面

网页源码仍在 `src/renderer`。改完后用 Node.js 22 构建一次，再重新编译 Rust，新的页面才会进到二进制里：

```bash
npm install
npm run build
cargo build --bin tianshell
```

`npm run build` 会把 `index.html`、`renderer.js`、`renderer.css` 写到 `ui/`。运行中的服务如果在仓库目录里启动，会优先读磁盘上的 `ui/`。

### 只跑 stdio MCP

```bash
cargo run --bin tianshell-mcp
```

这是给「客户端自己拉起进程」用的入口。工作台已经在跑时，一般不需要它；要附着到正在跑的那一份，用 `tianshell-mcp --attach`。细节在 [docs/mcp.md](docs/mcp.md)。

## 测试与类型检查

```bash
cargo test
npm test
npm run typecheck
npm run build
```

`cargo test` 覆盖鉴权、主机库、本机终端、工作台 MCP，以及 `tianshell --help` 不会启动服务。`npm test` 覆盖前端里广播目标和查找翻页这类纯逻辑。改界面后要先 `npm run build`，再编译 Rust，新页面才会进到二进制里。

## MCP 服务接入

工作台启动后 MCP 已经在听，不必再手动跑一遍 `tianshell-mcp`。Cursor、Claude Desktop、TianCode 的配置、令牌、stdio `--attach`，以及每个工具的参数和示例都写在 [docs/mcp.md](docs/mcp.md)。下面只留工具名单，避免和那份文档各写一套。

### 已暴露 MCP 工具列表

| 工具 | 作用 |
| :--- | :--- |
| `list_terminal_sessions` / `open_terminal` / `close_terminal` | 列出、打开或关闭交互终端。已有匹配标签时直接复用 |
| `read_terminal` / `write_terminal` / `interrupt_terminal` | 读当前屏幕、cwd、上次退出码；把按键打进这个标签；Ctrl+C |
| `run_command` | 优先打进已打开的标签并等待结果。没有标签，或 `new_session: true` 时才另开 SSH |
| `start_command` / `poll_command` / `cancel_command` | 在已打开的标签里跑长命令，不把调用方堵在一次请求上 |
| `search_history` | 搜索数据目录里的命令历史（时间、主机、命令、cwd、退出状态） |
| `list_hosts` / `list_groups` / `list_snippets` / `run_snippet` / `set_host_note` | 主机、分组、快捷命令和备注。不返回密码、私钥、口令或 MCP 令牌 |
| `list_tunnels` / `start_tunnel` / `stop_tunnel` | 查看或启停已经保存的端口转发。转发使用自己的连接 |
| `list_bindings` / `save_binding` | 记住本地项目路径对应哪台主机的哪个远端目录 |
| `sftp_list` / `sftp_read` / `sftp_write` / `sftp_download` / `sftp_upload` / `sftp_mkdir` | 文件操作另开一条 SSH，不占用交互终端那条线程 |
| `session_status` / `system_metrics` / `get_settings` | 一次性执行记录、采样，以及非机密设置 |

## 目录结构

```text
src/renderer  工作台网页（xterm.js、SFTP、监控、隧道、多标签）
src/shared    前后端共用的类型
ui            npm run build 生成的页面；Rust 进程优先读这里
rust          工作台、自动启动的 MCP、主机库与加密
docs/mcp.md   给其他 MCP 客户端看的接入说明
scripts       前端构建，以及 Windows 打包脚本
```

## 私钥格式

登录前会把私钥整理成 libssh2 / OpenSSL 能读的样子。直接粘贴下面这些都可以，加密的要在主机表单里填口令：

- OpenSSH（`BEGIN OPENSSH PRIVATE KEY`），包括 ed25519、RSA、ECDSA P-256 / P-384 / P-521。这是 Windows 自带 `ssh-keygen` 的默认格式；`ssh-keygen -p -m PEM` 对 ed25519 往往仍写出这种格式。
- PKCS#8（`BEGIN PRIVATE KEY`）。
- 传统 PEM RSA（`BEGIN RSA PRIVATE KEY`）。
- EC PEM（`BEGIN EC PRIVATE KEY`）。
- OpenSSL 加密 PEM（`BEGIN ENCRYPTED PRIVATE KEY`，或带 `Proc-Type: 4,ENCRYPTED` 的旧 PEM）。口令交给 libssh2，不在这里解密。

还不能用：PuTTY `.ppk`、FIDO / `sk-` 硬件密钥、只有证书没有私钥的文件。DSA 也不在这条路径里。

## 系统设计决策与安全防线规范 (Design Decisions & Security Guardrails)

TianShell 在架构与交互实现中，内置了一系列关键的**数据安全防护、连接多路复用与平台兼容规范**。这些策略属于系统 deliberate 的安全设计与边界防线：

### 🛡️ 数据安全与防误毁防线
- **符号链接安全处理**：删除符号链接只执行 `unlink` 解除链接自身，绝不顺着链接递归删除目标目录（避免误删 `/var/log` 等实际指向的系统敏感数据）；修改权限 (`chmod`) 作用于链接指向的目标对象权限位，权限输入框默认展示目标的实际权限位。
- **原子化安全上传**：文件上传先写入目标同目录下的独立临时文件（上限 64MB），全程使用目标文件的原有权限位；完整写入并校验成功后，通过 OpenSSH `posix-rename` 扩展进行原子替换。网络异常或传输中断绝不会产生半写损坏的原文件。
- **大文件截断保护**：在线文本编辑器与 MCP 读取严格限定 UTF-8 编码。对大于 256KB 的文本仅提供首段预览且**强制禁用写回**，彻底杜绝因在线编辑大日志文件导致原文件被意外截断损坏。
- **流式目录打包隔离**：目录打包通过远端 `tar -czf - -C <parent> -- <name>` 流式输出并转为 `.tar.gz` 下载，目录名置于 `--` 之后以防止短横线参数注入；禁止打包文件系统根目录 `/` 以避免遍历 `/proc` 等特殊设备导致死锁。

### ⚡ 连接多路复用与抗阻塞架构
- **三路连接物理隔离**：
  1. **交互终端独占连接**：键盘击键与屏幕回显独占主 SSH 连接，保证毫秒级敲击零延迟；
  2. **SFTP 与监控复用第二连接**：目录刷新、文件传输与 Bento 性能探针复用独立的副连接，传输中监控探针超时（5 秒）自动放弃，绝不阻塞用户终端操作；
  3. **MCP AI 自动化独立连接**：AI 助手（Cursor / Claude / TianCode）通过 MCP 执行批量文件操作时各自新建独立 SSH，互不抢占用户界面的操作通道。
- **有界容量淘汰 (LRU Bounded Eviction)**：会话管理与命令执行历史设置了严格的容量上限（已完成 Job 限制 200 条，映射索引限制 500 条），防止自动化长时间高频运行下内存泄漏。

### 🖥️ 平台边界与终端规范
- **Windows 原生 ConPTY 依赖**：Windows 下本机终端严格依赖 Win10 (1809+) / Win11 原生伪终端子系统（ConPTY），拒绝回退到易丢字符的陈旧管道模式。
- **标准化退出码提取**：打进交互终端的命令，标准输出与标准错误混在同一屏幕流中（真实 PTY 物理特性）；命令退出码通过远端 POSIX `$?` 与 Windows PowerShell `$LASTEXITCODE` 标准探针自动捕获。

## 许可

[MIT](LICENSE)
