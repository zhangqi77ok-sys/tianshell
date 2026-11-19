# TianShell

一个跨平台的 SSH/SFTP 协同工作台，内置标准 **MCP (Model Context Protocol)** 服务。既可以在浏览器里管理多台服务器，也可以让支持 MCP 的助手（TianCode / Cursor / Claude Desktop）连上同一套已保存的主机。

从这一版起，**启动工作台不再需要 Node.js**。`cargo run --bin tianshell -- serve` 由 Rust 进程提供页面、HTTP API、终端 WebSocket，并在同一个端口上自动启动 MCP。原来的 `npm start` 还在，它是上一版的 Node 工作台，用来对照和跑原有测试，不是现在的启动方式。界面仍然是仓库里的那套网页，由 Rust 进程送出去，没有把前端重写成另一套。

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
  - 同一个 SSH 标签里的交互终端、SFTP 和监控采样共用这一次登录。关掉标签时这条连接一起拆掉。端口转发仍然单独占一条连接，因为它要一直握着转发通道。
  - 同时开启多台 SSH 终端与本机 Shell（Windows PowerShell / POSIX `$SHELL`）标签页，互不关闭。本机终端走系统 PTY（Windows 上是 ConPTY），窗口尺寸会跟着变。
  - 支持终端内 `Ctrl+F` 搜索、拖拽调整上下分屏比例、勾选「广播输入」将命令/快捷片段同步下发到所有已打开终端。
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

启动工作台需要 Rust（stable）。不需要 Node.js，也不需要先执行 `npm install`。

```bash
cargo run --bin tianshell -- serve
```

进程绑定 `127.0.0.1`，默认端口 `53921`，并打开浏览器。只要看地址、不打开浏览器：

```bash
cargo run --bin tianshell -- serve --no-open
```

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

解压后双击 `tianshell.exe` 就会启动工作台并打开浏览器。窗口里的控制台要留着，关掉它进程就停了。想进开始菜单：

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

默认构建不包含桌面窗口。需要系统 WebView 时再打开 `desktop` 特性（Windows 用 WebView2，Linux 还要有 WebKitGTK）：

```bash
cargo run --features desktop --bin tianshell
```

窗口打开的就是上面同一个 Rust 服务，不会再去启动 Node。

### 改界面

网页源码仍在 `src/renderer`。改完后用 Node.js 22 构建一次，再重新编译 Rust，新的页面才会进到二进制里：

```bash
npm install
npm run build
cargo build --bin tianshell
```

`npm run build` 会把 `index.html`、`renderer.js`、`renderer.css` 复制到 `ui/`。运行中的服务如果在仓库目录里启动，会优先读磁盘上的 `ui/`。

### 原来的 Node 工作台

`npm start` 仍然会编译并启动 `dist/server.js`。那是 0.2 的 Node 实现，保留下来给原有测试和对照。不要和 Rust 进程同时写同一个 `TIANSHELL_HOME`。

### 只跑 stdio MCP

```bash
cargo run --bin tianshell-mcp
```

这是给「客户端自己拉起进程」用的入口。工作台已经在跑时，一般不需要它；要附着到正在跑的那一份，用 `tianshell-mcp --attach`。细节在 [docs/mcp.md](docs/mcp.md)。

## 测试与类型检查

```bash
cargo test
npm run typecheck
npm test
```

`cargo test` 覆盖 Rust 工作台的鉴权、主机/片段/设置接口、本机终端会话，以及工作台起来时 MCP 已经可调用。`npm test` 仍覆盖原来的 Node 实现，包括本机 SSH/SFTP。

## MCP 服务接入

工作台启动后 MCP 已经在听，不必再手动跑一遍 `tianshell-mcp`。Cursor、Claude Desktop、TianCode 的配置、令牌、stdio `--attach`，以及每个工具的参数和示例都写在 [docs/mcp.md](docs/mcp.md)。下面只留工具名单，避免和那份文档各写一套。

### 已暴露 MCP 工具列表

| 工具 | 作用 |
| :--- | :--- |
| `list_hosts` | 列出已保存主机、分组与跳板机关联（不含密码与私钥） |
| `list_terminal_sessions` | 列出工作台里已经打开的交互终端。stdio 独立进程没有附着工作台时，这个列表是空的 |
| `run_command` | 另开一条 SSH（支持 `ProxyJump`）执行命令，返回输出与退出码。不往界面终端里写 |
| `session_status` | 查看 `run_command` 留下的记录，不含界面终端 |
| `sftp_list` | 列出远端 POSIX 目录内容与权限 |
| `sftp_mkdir` | 新建远端目录 |
| `sftp_read` | 只读读取远端 UTF-8 文本/配置文件（上限 256KB） |
| `sftp_write` | 将 UTF-8 文本写回远端文件（上限 512KB） |
| `system_metrics` | 采样并返回目标主机 CPU / 内存 / Swap / 磁盘 / 负载 / Top 进程 |

## 目录结构

```text
src/core      分组主机库、DPAPI 适配器 (TS1D)、AES-GCM、片段/隧道/设置
src/ssh       Node 版 SSH、ProxyJump、SFTP、监控、L/R/SOCKS5、本机终端
src/mcp       Node 版 MCP（stdio）
src/web       Node 版 HTTP / WebSocket 工作台
src/renderer  工作台网页（xterm.js、SFTP、监控、隧道、多标签）
ui            已构建的网页，由 Rust 进程提供；npm run build 会更新它
rust          Rust 工作台、自动启动的 MCP、主机库与加密
docs/mcp.md   给其他 MCP 客户端看的接入说明
test          Node 版单元测试和本机 SSH/SFTP 集成测试
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

- 同一个 SSH 标签的终端、SFTP 和监控共用一条连接。大文件传输会占住这条连接，传完之前终端会停一下。端口转发仍然是另一条连接。
- 上传先收下整个请求体（上限 64MB）再写入远端；下载是分块送出的。
- 本地转发、SOCKS5 和远程转发的代码在 Rust 里。这次的 Rust 测试没有启动 SSH 服务，所以没有做真实的远端隧道或 SFTP 往返。那些真实连接仍由 Node 测试覆盖。
- MCP 的 `run_command` 和界面里的交互终端仍是两套会话。前者每次自己登录；后者可以用 `list_terminal_sessions` 看到，但不会被写进去。
- 本机终端在 Windows 上依赖 ConPTY。更老的、没有 ConPTY 的系统打不开本机标签，而不是退回管道。

## 许可

[MIT](LICENSE)
