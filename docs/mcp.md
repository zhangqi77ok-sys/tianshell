# TianShell MCP

TianShell 的工作台进程启动时会**自动**在本机拉起 MCP，不需要再手动执行一次 `tianshell-mcp`。其他工具连的是已经在跑的这一份，这样看到的主机库和 `TIANSHELL_HOME` 与界面是同一套。

英文说明在文末 [For MCP client authors](#for-mcp-client-authors)。

## 启动

```bash
cargo run --bin tianshell -- serve
```

这条命令只启动 Rust 进程。它不会调用 Node.js，也不会去找 `dist/server.js`。

默认监听 `127.0.0.1:53921`。成功后终端会打印界面地址和 MCP 地址，并在数据目录写入 `runtime.json`。关掉进程时这个文件会被删掉，避免客户端连到一个已经退出的端口。

常用环境变量：

| 变量 | 作用 |
| --- | --- |
| `TIANSHELL_HOME` | 数据目录。界面和 MCP 必须用同一个目录，才能看见同一批主机。 |
| `TIANSHELL_PORT` | 监听端口。默认 `53921`。设为 `0` 时由系统分配。 |
| `TIANSHELL_TOKEN` | 固定本机访问令牌。不设的话，第一次启动会生成并写入 `mcp.token`，以后一直沿用。 |
| `TIANSHELL_UI` | 可选。改用这个目录里的 `index.html` / `renderer.js` / `renderer.css`，而不是二进制里嵌的那一份。 |

数据目录默认是：

- Windows：`%APPDATA%\tianshell`
- macOS：`~/Library/Application Support/tianshell`
- Linux：`~/.config/tianshell`

`runtime.json` 的样子：

```json
{
  "pid": 12345,
  "port": 53921,
  "workbenchUrl": "http://127.0.0.1:53921/?token=…",
  "mcpUrl": "http://127.0.0.1:53921/mcp",
  "sseUrl": "http://127.0.0.1:53921/sse",
  "token": "…",
  "protocolVersion": "2025-03-26",
  "startedAt": "2026-11-19T03:45:00.000Z"
}
```

令牌也在 `mcp.token` 里，文件权限是仅当前用户可读（Windows 上靠用户配置文件的 ACL）。它和界面用的 `x-tianshell-token` 是同一个值。

## 怎么连

只监听环回地址。请求要带令牌，三者任一即可：

- `Authorization: Bearer <token>`（推荐）
- `x-tianshell-token: <token>`
- 查询参数 `?token=<token>`

带了 `Origin` 或 `Host` 时，只接受 `localhost` / `127.0.0.1`。这是为了挡住浏览器里的 DNS 重绑定。

工作台已经在跑时，优先用下面的 **Streamable HTTP**。stdio 仍然保留，给那些只能自己拉起进程的客户端。

### 1. Streamable HTTP（推荐）

当前协议是 [MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports)（也接受客户端申报的 `2024-11-05` 和 `2025-06-18`）。

- `POST http://127.0.0.1:53921/mcp`：一条 JSON-RPC 请求，响应 `application/json`
- `GET http://127.0.0.1:53921/mcp`：打开 SSE。这个服务没有服务器主动消息，这条流只用来保活
- `DELETE http://127.0.0.1:53921/mcp`：结束 `Mcp-Session-Id` 对应的会话
- `initialize` 的响应头里有 `Mcp-Session-Id`。之后的请求带上它

没有 `id` 的通知返回 `202`，没有响应体。

#### Cursor

```json
{
  "mcpServers": {
    "tianshell": {
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer 把 mcp.token 里的内容贴这里"
      }
    }
  }
}
```

#### Claude Desktop / 其他支持 HTTP MCP 的客户端

```json
{
  "mcpServers": {
    "tianshell": {
      "type": "http",
      "url": "http://127.0.0.1:53921/mcp",
      "headers": {
        "Authorization": "Bearer 把 mcp.token 里的内容贴这里"
      }
    }
  }
}
```

先启动 TianShell，再让客户端连接。端口或令牌改过之后，改这里的配置。

### 2. 旧版 HTTP + SSE

还在用 2024-11-05 SSE 传输的客户端：

- `GET http://127.0.0.1:53921/sse` 会先收到 `event: endpoint`，`data` 是 `/message?sessionId=…`
- 客户端把 JSON-RPC `POST` 到那个地址。结果从 SSE 的 `event: message` 回来，HTTP 本身返回 `202`

```json
{
  "mcpServers": {
    "tianshell": {
      "url": "http://127.0.0.1:53921/sse",
      "headers": {
        "Authorization": "Bearer 把 mcp.token 里的内容贴这里"
      }
    }
  }
}
```

### 3. stdio

有的客户端只会自己启动一个进程。可以继续用独立的 stdio 服务（它自己读 `TIANSHELL_HOME`，不要求工作台先开着）：

```bash
cargo build --release --bin tianshell-mcp
```

```json
{
  "mcpServers": {
    "tianshell": {
      "command": "C:\\path\\to\\tianshell\\target\\release\\tianshell-mcp.exe",
      "env": {
        "TIANSHELL_HOME": "D:\\data\\tianshell"
      }
    }
  }
}
```

如果工作台已经开着，希望这个 stdio 进程只是转接到那一份 MCP（主机列表和 `session_status` 跟界面进程是同一份），加上 `--attach`：

```json
{
  "mcpServers": {
    "tianshell": {
      "command": "C:\\path\\to\\tianshell\\target\\release\\tianshell-mcp.exe",
      "args": ["--attach"],
      "env": {
        "TIANSHELL_HOME": "D:\\data\\tianshell"
      }
    }
  }
}
```

`--attach` 读取同一个数据目录里的 `runtime.json`，把每一行 JSON-RPC 转到 `mcpUrl`。工作台没开时它会在 stderr 说明原因并退出，不会悄悄再起一个互不相干的服务。

日志走 stderr。stdio 模式下 stdout 只写 JSON-RPC。

## 工具

七个工具。`host` 可以是主机 id，也可以是别名（不区分大小写）。密码和私钥不会出现在返回值里。

命令会在远程机器上真实执行。只把 TianShell 接到你信任的助手上。

### `list_hosts`

列出已保存的主机，不含密文。

```json
{ "name": "list_hosts", "arguments": {} }
```

### `run_command`

经 ProxyJump 链连上主机，执行一条命令。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `host` | 是 | 主机 id 或别名 |
| `command` | 是 | 单条命令，最长 8000 字符，不能含空字符 |
| `timeout_seconds` | 否 | 默认 30，范围 1–120 |

```json
{
  "name": "run_command",
  "arguments": { "host": "lab", "command": "uname -a", "timeout_seconds": 30 }
}
```

返回 JSON 文本，字段包括 `sessionId`、`exitCode`、`timedOut`、`truncated`、`stdout`、`stderr`。标准输出和标准错误各自最多保留约 20 万字符。

### `session_status`

查看**这个 MCP 进程**里 `run_command` 留下的记录。界面上打开的交互终端不在这里。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `session_id` | 否 | 不传则返回全部 |

```json
{ "name": "session_status", "arguments": { "session_id": "…" } }
```

### `sftp_list`

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `host` | 是 | 主机 id 或别名 |
| `path` | 否 | POSIX 路径，默认 `/` |

```json
{ "name": "sftp_list", "arguments": { "host": "lab", "path": "/etc" } }
```

### `sftp_read`

只读 UTF-8 文本。二进制文件会拒绝。上限 256KB，默认最多读 131072 字节。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `host` | 是 | 主机 id 或别名 |
| `path` | 是 | 远端文件 |
| `max_bytes` | 否 | 256–262144，默认 131072 |

返回 JSON：`{"content":"…","truncated":false}`。

### `sftp_write`

覆盖写入 UTF-8 文本。上限 512KB。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `host` | 是 | 主机 id 或别名 |
| `path` | 是 | 远端文件 |
| `content` | 是 | 完整内容 |

```json
{
  "name": "sftp_write",
  "arguments": {
    "host": "lab",
    "path": "/tmp/tianshell-note.txt",
    "content": "hello\n"
  }
}
```

### `system_metrics`

对目标主机做一次采样，返回 CPU、内存、Swap、磁盘、网卡计数和 Top 进程。CPU 占用和网卡速率用两次采样的差值，中间大约间隔 200ms。

```json
{ "name": "system_metrics", "arguments": { "host": "lab" } }
```

## 和界面的关系

- 在界面里新增或修改的主机，同一个进程里的 MCP 马上能 `list_hosts` 到。两边用的是同一把 AES-256-GCM 钥匙。
- Windows 上 `vault.key` 仍是 `TS1D` + DPAPI。Linux 和 macOS 上新写入的钥匙是 `TS1R` 加 32 字节原始钥匙，和 Node 版在没有系统密钥环时的格式一致，旧的 `TS1D` 文件也能读。
- 界面里的交互终端，和 MCP 的 `run_command` 不是同一个会话。

不要让 Node 版 `npm start` 和这个 Rust 进程同时写同一个 `TIANSHELL_HOME`。

## For MCP client authors

TianShell starts MCP inside the workbench process. Point clients at the running server. Do not require a second manual launch.

Start:

```bash
cargo run --bin tianshell -- serve
```

Default endpoint: `http://127.0.0.1:53921/mcp` (Streamable HTTP, spec revisions `2025-03-26`, `2025-06-18`, and `2024-11-05`). Legacy SSE is `GET /sse` plus `POST /message?sessionId=…`.

Auth: `Authorization: Bearer <token>`. The token is stable for a data directory (`TIANSHELL_TOKEN`, otherwise `mcp.token` under `TIANSHELL_HOME`). The same value is in `runtime.json` after start. Bind address is loopback only.

`initialize` returns `Mcp-Session-Id`. Send it on later requests. Notifications (no `id`) get HTTP 202.

stdio binary: `tianshell-mcp`. Add `--attach` to proxy stdin/stdout to the already running workbench (`runtime.json` in `TIANSHELL_HOME`). Without `--attach`, stdio is a standalone server on the same data directory.

Tools: `list_hosts`, `run_command`, `session_status`, `sftp_list`, `sftp_read`, `sftp_write`, `system_metrics`. Parameters match the tables above. `list_hosts` never returns passwords or private keys. `run_command` really executes on the saved host, including ProxyJump.

Copy-paste client config is in the Chinese sections above; only the token and, if you changed it, the port need editing.
