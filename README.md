# TianShell

一个跨平台的 SSH/SFTP 协同工作台，内置标准 **MCP (Model Context Protocol)** 服务。既可以在浏览器里管理多台服务器，也可以让支持 MCP 的助手（TianCode / Cursor / Claude Desktop）连上同一套已保存的主机。

从这一版起，**启动工作台不再需要 Node.js**。`cargo run --bin tianshell` 默认打开原生窗口；`cargo run --bin tianshell -- serve` 只在浏览器里打开。Rust 进程提供页面、HTTP API、终端 WebSocket，并在同一个端口上自动启动 MCP。界面源码在 `src/renderer`，`npm run build` 生成 `ui/` 后由这个进程送出去。

接入 MCP 的完整说明在 [docs/mcp.md](docs/mcp.md)。

作者是 zq（[zhangqi77ok-sys](https://github.com/zhangqi77ok-sys)）。

## 核心特性

- **Rust 工作台，网页界面仍由这个进程提供**：
  - `cargo run --bin tianshell -- serve` 不启动 Node，也不再打包 Electron。
  - 凭据使用 **AES-256-GCM** 加密存储；Windows 下通过系统原生 **DPAPI (`CurrentUser`, `TS1D` 封装)** 保护 `vault.key`，POSIX 下新的钥匙文件是 `TS1R`（仅当前用户可读），旧的 `TS1D` / Node 写出的 `TS1R` 都能读。
- **主机分组、标签与 ProxyJump 多跳跳板机**：
  - 左侧支持自定义分组、关键词/标签搜索过滤、密码与私钥认证、TOFU 主机密钥指纹记忆与清除。私钥可以直接粘贴日常使用的 OpenSSH 格式（含 ed25519，可以带口令），也可以用 PKCS#8、PEM RSA 或 EC PEM。
  - 主机可配置 `jumpHostId` 跳板机链路（内置循环依赖检测），通过多跳 `forwardOut` 直达内网节点。
- **多标签终端 & 本机 Shell & 广播输入**：
  - 同一个 SSH 标签在界面上仍是一个会话。交互终端独占一条 SSH 连接；SFTP 和监控走同一条不另开标签的第二条连接，所以采样、列目录和大文件传输不会挡住键盘和回显。关掉标签时两条连接一起拆掉。端口转发仍然单独占一条连接，因为它要一直握着转发通道。
  - 同时开启多台 SSH 终端与本机 Shell（Windows PowerShell / POSIX `$SHELL`）标签页，互不关闭。本机终端走系统 PTY（Windows 上是 ConPTY），窗口尺寸会跟着变。
  - 终端、文件和监控共用一条会话标签，可以不离开当前视图换会话。`Ctrl+F` 在终端里查找。广播默认关闭；打开后按键进入当前终端，以及标签上另外勾选且仍连接的会话。本机终端和已断开的标签默认不勾选。
- **完整 SFTP 文件管理与在线文本编辑**：
  - 支持目录浏览、面包屑导航、显示隐藏文件、新建目录、重命名、权限修改 (`chmod`)、递归删除。
  - 支持文件**拖拽上传**、流式下载、底部实时传输队列进度与取消；双击文本/配置文件可弹出等宽编辑器直接修改并写回远端。
- **服务器实时 Bento 监控仪表盘**：
  - 定时低开销采集主机 CPU %、内存 & Swap 占用、网络上下行速率（RX/TX B/s）、LoadAvg、磁盘分区及 Top CPU 进程榜（高负载自动告警变色）。
- **SSH 端口转发与 SOCKS5 动态代理**：
  - 支持 `-L`（本地转发）、`-R`（远程反向转发）及内置 RFC 1928 **-D 动态 SOCKS5 代理**，规则持久化且一键启停。
- **快捷命令库与暗/亮双主题**：
  - 内置常用 Linux 诊断命令卡片并支持自定义分类 CRUD、暗/亮双主题、终端字号与采样周期持久化配置。

## 环境与运行

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

## 这一步还没有完全对齐的地方

- 大文件 SFTP 传输和监控采样共用第二条连接。传输没结束时，下一次采样和目录刷新会等着，终端输入和回显不会停。监控每次只跑一遍探针，CPU 和网卡速率用相邻两次的差值；第一拍还没有差值，CPU 显示 0、速率显示 0。探针超过 5 秒会放弃这一拍，避免卡住的 `df` / `ps` 一直占着这条 SFTP 连接。只允许一条并发会话的主机会拒绝这次登录。端口转发仍然是另一条连接。
- 删除符号链接只去掉链接，不会顺着它删除指向的目录。真实目录的删除仍然是递归的。点链接名会先尝试列出它指向的目录；列不开时再按文件打开。改权限仍作用在链接指向的对象上。
- 上传先收下整个请求体（上限 64MB），再写到同目录的临时文件，全部写完才换上原来的文件。已有文件的临时文件从创建起就用原来的权限位。换上已有文件时走 OpenSSH 的 posix-rename。第 3 版普通改名不会覆盖已有普通文件，会返回 SFTP(4)，原文件还在。没有这个扩展时不先删旧文件，上传失败，原文件仍保持不变。没有斜杠的相对路径相对当前目录，不会写到文件系统根。下载是分块送出的。浏览器下载先换一张一次性票，地址里不放长期访问令牌。打包下载不会把整个文件系统根目录 `/` 打成一个包。目录名放在 `tar --` 后面，避免以 `-` 开头的名字被当成选项。远端没有 `tar`，或 `tar` 退出码不是 0 时，这次打包记为失败。
- 本地转发、SOCKS5 和远程转发的代码在 Rust 里。Rust 测试没有做真实的远端隧道往返。已有文件的覆盖会在本机有 sshd、并且能免密 sudo 时，连到只监听 127.0.0.1 的 OpenSSH 上验证（登录用户是本机的 `tianshellsftp`）；没有 sshd 时这条测试不跑。
- MCP 的文件工具仍各自另开一条 SSH，不进入交互终端，也不占用标签上那条 SFTP/监控连接。工作台里的目录刷新和传输走标签的第二条连接。
- 打进交互终端的命令，标准输出和标准错误混在同一份屏幕里。退出码来自一段收尾：远端和 POSIX 本机用 `$?`，Windows 本机 PowerShell 用 `$LASTEXITCODE`。`fish` / `csh` 读不懂这段收尾，这时 `exitCode` 为空，屏幕文本仍然返回。
- 交互历史记的是 TianShell 送进终端的那一行。Windows ConPTY 里，只发生在控制台内部的行编辑（例如方向键翻历史）不会被还原成命令。
- 本机终端在 Windows 上依赖 ConPTY。更老的、没有 ConPTY 的系统打不开本机标签，而不是退回管道。

## 许可

[MIT](LICENSE)
