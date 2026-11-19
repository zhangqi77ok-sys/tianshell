# TianShell

一个跨平台的 SSH/SFTP 协同工作台，内置标准 **MCP (Model Context Protocol)** 服务。既可以在浏览器或原生窗口里管理多台服务器，也可以让支持 MCP 的 AI 助手（TianCode / Cursor / Claude Desktop）通过 `stdio` 协同排查与运维。

工作台本身是 **纯 Node.js 22**（已经不再依赖 Electron）。Windows 上还可以再用 Rust 编一个原生窗口，窗口里仍然打开同一套工作台。另有一个纯 Rust 的 MCP 入口，可与 Node 版二选一。

作者是 zq（[zhangqi77ok-sys](https://github.com/zhangqi77ok-sys)）。

## 核心特性

- **纯 Node.js 22 轻量架构（已彻底脱离 Electron）**：
  - 安装与冷启动极快（`dist/server.js` + `dist/mcp.js`），不再打包 Chromium/Electron。
  - 凭据使用 **AES-256-GCM** 加密存储；Windows 下通过系统原生 **DPAPI (`CurrentUser`, `TS1D` 封装)** 保护 `vault.key`，POSIX 下按 `0600` 权限保护。
- **主机分组、标签与 ProxyJump 多跳跳板机**：
  - 左侧支持自定义分组、关键词/标签搜索过滤、密码与 OpenSSH/PEM 私钥认证、TOFU 主机密钥指纹记忆与清除。
  - 主机可配置 `jumpHostId` 跳板机链路（内置循环依赖检测），通过多跳 `forwardOut` 直达内网节点。
- **多标签终端 & 本机 Shell & 广播输入**：
  - 同时开启多台 SSH 终端与本机 Shell（Windows PowerShell / POSIX `$SHELL`）标签页，互不关闭。
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

需要 **Node.js 22 或更高版本**。

```bash
npm install
npm start
```

`npm start` 会自动编译构建并启动本地工作台（绑定 `127.0.0.1` + 一次性会话 Token 鉴权）并打开浏览器访问。只构建、不打开浏览器可执行 `node dist/server.js --no-open`。

### 原生窗口（Windows）

先构建工作台，再启动桌面窗口。窗口会自动拉起同一套本地服务；如果找不到 `dist/server.js` 或本机没有 Node.js，会在窗口里说明原因，而不是停在空白页。

```bash
npm install
npm run build
cargo run --bin tianshell
```

发布用的可执行文件：

```bash
cargo build --release --bin tianshell
```

生成的是 `target/release/tianshell.exe`（本机若使用 GNU 工具链，路径可能带目标三元组）。

### 纯 Rust MCP

```bash
cargo run --bin tianshell-mcp
```

它和 `node scripts/mcp.mjs` 一样走 stdio，读取同一套本机主机数据。两边不要同时抢同一个数据目录里的写入。

## 测试与类型检查

```bash
npm run typecheck
npm test
cargo test
```

## MCP 服务接入

在支持 MCP 的客户端（如 TianCode / Cursor）中，可以用 Node 入口：

```json
{
  "mcpServers": {
    "tianshell": {
      "command": "node",
      "args": ["scripts/mcp.mjs"],
      "cwd": "C:\\path\\to\\tianshell"
    }
  }
}
```

也可以改成 Rust 入口（先 `cargo build --release --bin tianshell-mcp`）：

```json
{
  "mcpServers": {
    "tianshell": {
      "command": "C:\\path\\to\\tianshell\\target\\release\\tianshell-mcp.exe"
    }
  }
}
```

如需独立数据目录，可传入 `"env": { "TIANSHELL_HOME": "D:\\data\\tianshell" }`。日志统一输出至 `stderr`，`stdout` 仅保留 JSON-RPC 2.0 协议帧。

### 已暴露 MCP 工具列表

| 工具 | 作用 |
| :--- | :--- |
| `list_hosts` | 列出已保存主机、分组与跳板机关联（不含密码与私钥） |
| `run_command` | 连上目标主机（支持 `ProxyJump`）执行命令，返回输出与退出码 |
| `session_status` | 查看本 MCP 进程内的命令执行会话记录 |
| `sftp_list` | 列出远端 POSIX 目录内容与权限 |
| `sftp_read` | 只读读取远端 UTF-8 文本/配置文件（上限 256KB） |
| `sftp_write` | 将 UTF-8 文本写回远端文件（上限 512KB） |
| `system_metrics` | 采样并返回目标主机 CPU / 内存 / Swap / 磁盘 / 负载 / Top 进程 |

## 目录结构

```text
src/core      分组主机库 (HostStore)、DPAPI 适配器 (TS1D)、AES-GCM 仓库、快照/隧道/设置仓库
src/ssh       SSH 连接/ProxyJump 链路、SFTP 流式操作、系统监控采样、L/R/SOCKS5 隧道、本机终端
src/mcp       纯 Node MCP 工具注册与 stdio CLI 入口
src/web       纯 Node HTTP REST API、原生 RFC 6455 WebSocket Hub 与服务入口
src/renderer  四区复刻工作台前端 UI (xterm.js + SFTP/监控/隧道/多标签控制)
rust          原生窗口、纯 Rust MCP，以及与 Node 共用的主机库/加密实现
test          单元与真实本地 SSH/SFTP/WebSocket/MCP 集成测试
```

## 许可

[MIT](LICENSE)
