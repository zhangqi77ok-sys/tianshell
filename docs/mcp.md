# TianShell MCP

工作台进程启动时会**自动**在本机拉起 MCP。Cursor、Claude Code 和其他支持 MCP 的工具连的是这一份，主机库和 `TIANSHELL_HOME` 与界面相同。

英文说明在文末 [For MCP client authors](#for-mcp-client-authors)。

## 启动

```bash
cargo run --bin tianshell -- serve
```

Windows 安装包里双击 `tianshell.exe` 就是这条命令，不用再记参数。见仓库 README 的「Windows 安装包」。

默认监听 `127.0.0.1:53921`。成功后终端打印界面地址和 MCP 地址（不打印令牌），并在数据目录写入 `runtime.json`。关掉进程时这个文件会被删掉。

| 变量 | 作用 |
| --- | --- |
| `TIANSHELL_HOME` | 数据目录。界面和 MCP 必须用同一个目录。 |
| `TIANSHELL_PORT` | 监听端口。默认 `53921`。设为 `0` 时由系统分配。 |
| `TIANSHELL_TOKEN` | 固定本机访问令牌。不设的话，第一次启动会生成并写入 `mcp.token`，以后一直沿用。 |
| `TIANSHELL_UI` | 可选。改用这个目录里的页面文件。 |

数据目录默认是：

- Windows：`%APPDATA%\tianshell`
- macOS：`~/Library/Application Support/tianshell`
- Linux：`~/.config/tianshell`

令牌在 `mcp.token`，也在工作台启动后的 `runtime.json` 里。界面左下角的「连接 MCP」会显示地址和令牌，并给出可以直接复制的客户端配置。不要把令牌写进仓库或日志。

## 客户端该用哪种传输

| 客户端 | 用这个 | 原因 |
| --- | --- | --- |
| Cursor | Streamable HTTP，`url` + `headers`，**不要**写 `type` | Cursor 看到 `url` 就走 HTTP / SSE |
| Claude Code | Streamable HTTP，必须写 `"type": "http"` | 只有 `url`、没有 `type` 时，Claude Code 会把它当成 stdio 并跳过 |
| 只支持 2024-11-05 SSE 的旧客户端 | `GET /sse` | 服务仍提供这条旧传输 |
| 只会自己拉起进程的客户端 | `tianshell-mcp --attach` | stdio 转到已经在跑的工作台 |

先启动 TianShell，再让客户端连接。端口或令牌变了，就改配置。下面的 `YOUR_TOKEN` 换成 `mcp.token` 里的内容，或从界面「连接 MCP」直接复制。

### Cursor

写到 `~/.cursor/mcp.json`（所有项目）或项目里的 `.cursor/mcp.json`。

```json
{
  "mcpServers": {
    "tianshell": {
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

不想把令牌写进文件时，先在启动工作台的环境里设置 `TIANSHELL_TOKEN`，再让 Cursor 读同一个变量：

```json
{
  "mcpServers": {
    "tianshell": {
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer ${env:TIANSHELL_TOKEN}"
      }
    }
  }
}
```

保存后重载 MCP。Cursor 的 `${env:NAME}` 只在 Cursor 里展开，和 Claude Code 的写法不一样。

### Claude Code

`type` 不能省。项目级配置是仓库根目录的 `.mcp.json`，个人配置用 `claude mcp add`。

```json
{
  "mcpServers": {
    "tianshell": {
      "type": "http",
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

命令行（用户级，所有项目）：

```bash
claude mcp add --transport http --scope user tianshell http://127.0.0.1:53921/mcp \
  --header "Authorization: Bearer YOUR_TOKEN"
```

Claude Code 会展开 `${TIANSHELL_TOKEN}`，语法不是 Cursor 的 `${env:...}`：

```json
{
  "mcpServers": {
    "tianshell": {
      "type": "http",
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer ${TIANSHELL_TOKEN}"
      }
    }
  }
}
```

`streamable-http` 是 `http` 的别名，写哪一个都可以。

### 旧版 HTTP + SSE

还在用 2024-11-05 SSE 的客户端：

- `GET http://127.0.0.1:53921/sse` 先收到 `event: endpoint`，`data` 是 `/message?sessionId=…`
- JSON-RPC `POST` 到那个地址。结果从 SSE 的 `event: message` 回来，HTTP 本身返回 `202`

Claude Code：

```json
{
  "mcpServers": {
    "tianshell": {
      "type": "sse",
      "url": "http://127.0.0.1:53921/sse",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

Cursor 把 SSE 和 Streamable HTTP 都放在 `url` 上。优先用上面的 `/mcp`。只有客户端明确要求 SSE 端点时才用 `/sse`。

### stdio

客户端只会自己启动进程时，用 `tianshell-mcp`。工作台已经开着时加上 `--attach`，这样主机列表和界面终端是同一份。工作台没开时它会在 stderr 说明原因并退出，不会再偷偷起一个互不相干的服务。

```bash
cargo build --release --bin tianshell-mcp
```

Cursor：

```json
{
  "mcpServers": {
    "tianshell": {
      "command": "C:\\Users\\you\\AppData\\Local\\TianShell\\tianshell-mcp.exe",
      "args": ["--attach"]
    }
  }
}
```

Claude Code 必须写 `type`：

```json
{
  "mcpServers": {
    "tianshell": {
      "type": "stdio",
      "command": "C:\\Users\\you\\AppData\\Local\\TianShell\\tianshell-mcp.exe",
      "args": ["--attach"]
    }
  }
}
```

路径改成安装位置。默认数据目录不用设 `TIANSHELL_HOME`。两边如果改过数据目录，环境变量必须相同。

不带 `--attach` 的 stdio 是独立进程：能读写同一份主机库，但看不到界面里已经打开的终端。

日志走 stderr。stdio 模式下 stdout 只写 JSON-RPC。

## 协议细节

当前推荐的是 [MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports)。也接受客户端申报的 `2024-11-05` 和 `2025-06-18`。

- `POST http://127.0.0.1:53921/mcp`：一条 JSON-RPC，响应 `application/json`
- `GET http://127.0.0.1:53921/mcp`：SSE 保活。这个服务没有服务器主动消息
- `DELETE http://127.0.0.1:53921/mcp`：结束 `Mcp-Session-Id`
- `initialize` 的响应头里有 `Mcp-Session-Id`。之后的请求带上它
- 没有 `id` 的通知返回 `202`，没有响应体

只监听环回地址。令牌三者任一即可：

- `Authorization: Bearer <token>`（推荐）
- `x-tianshell-token: <token>`
- 查询参数 `?token=<token>`

带了 `Origin` 或 `Host` 时，只接受 `localhost` / `127.0.0.1`。

## 工具

命令会在远程机器上真实执行。只把 TianShell 接到你信任的助手上。`host` 可以是主机 id 或别名（不区分大小写）。密码和私钥不会出现在返回值里。

### `list_hosts`

列出已保存的主机，不含密文。

### `list_terminal_sessions`

列出**工作台界面里**已经打开的交互终端（别名、主机、`sessionId`、`kind` 是 `ssh` 或 `local`）。

没有附着到工作台时（单独跑 `tianshell-mcp` 且不加 `--attach`）返回 `attached: false` 和空列表。

### `run_command`

另开一条 SSH（含 ProxyJump）执行一条命令。**不会**把按键写进界面里已经打开的终端，也**不**复用那个标签的连接。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `host` | 是 | 主机 id 或别名 |
| `command` | 是 | 单条命令，最长 8000 字符，不能含空字符 |
| `timeout_seconds` | 否 | 默认 30，范围 1–120 |

返回 JSON 文本：`sessionId`、`exitCode`、`timedOut`、`truncated`、`stdout`、`stderr`。标准输出和标准错误各自最多保留约 20 万字符。

### `session_status`

只看 `run_command` 留下的记录。不传 `session_id` 则返回全部。界面终端不在这里，用 `list_terminal_sessions`。

### `sftp_list` / `sftp_mkdir` / `sftp_read` / `sftp_write`

这些调用各自再登录一次，不占用界面标签那条连接。

| 工具 | 参数 | 说明 |
| --- | --- | --- |
| `sftp_list` | `host`，可选 `path`（默认 `/`） | 列目录 |
| `sftp_mkdir` | `host`，`path` | 新建目录。拒绝空路径和 `/` |
| `sftp_read` | `host`，`path`，可选 `max_bytes` | 只读 UTF-8。256–262144，默认 131072。二进制会拒绝 |
| `sftp_write` | `host`，`path`，`content` | 覆盖写入 UTF-8，上限 512KB |

### `system_metrics`

对目标主机另开一条连接做一次采样，返回 CPU、内存、Swap、磁盘、网卡和 Top 进程。CPU 和网卡速率用两次采样的差值，中间大约间隔 200ms。

界面标签上的监控走的是那条已经登录的连接，和这个工具不是同一次采样。

## 和界面的关系

- 界面里新增或修改的主机，同一个进程里的 MCP 马上能 `list_hosts` 到。
- 一个 SSH 标签的终端、SFTP 和监控共用一次登录。关掉标签时一起拆掉。
- `run_command` 和界面终端是两套会话。要看界面上开了哪些标签，调用 `list_terminal_sessions`。
- 不要让 Node 版 `npm start` 和这个 Rust 进程同时写同一个 `TIANSHELL_HOME`。

## For MCP client authors

TianShell starts MCP inside the workbench. Point clients at the running server.

```bash
cargo run --bin tianshell -- serve
```

Preferred transport: Streamable HTTP `POST/GET/DELETE http://127.0.0.1:53921/mcp`. Spec revisions `2025-03-26`, `2025-06-18`, and `2024-11-05`. Legacy SSE is `GET /sse` plus `POST /message?sessionId=…`.

Auth: `Authorization: Bearer <token>`. The token is stable for a data directory (`TIANSHELL_TOKEN`, otherwise `mcp.token`). The same value is in `runtime.json` while the process is up. Bind address is loopback only. `initialize` returns `Mcp-Session-Id`.

Cursor config is `url` plus `headers`, with no `type` field. Claude Code config must set `"type": "http"` (alias `"streamable-http"`). A Claude Code entry that has `url` and no `type` is skipped. Copy-paste JSON is in the sections above; replace `YOUR_TOKEN` or read it from the workbench **连接 MCP** dialog. Do not commit the token.

stdio binary: `tianshell-mcp --attach` proxies to the running workbench. Without `--attach`, stdio is a separate server on the same data directory and cannot see interactive tabs.

Tools: `list_hosts`, `list_terminal_sessions`, `run_command`, `session_status`, `sftp_list`, `sftp_mkdir`, `sftp_read`, `sftp_write`, `system_metrics`. `run_command` opens its own SSH session and does not write into an interactive terminal tab. `list_terminal_sessions` lists those tabs when the handler is attached to the workbench. `session_status` lists `run_command` records only.
