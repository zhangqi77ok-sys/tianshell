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

工作台「连接 MCP」里的「写入 Cursor 配置」会在确认后改用户目录的 `~/.cursor/mcp.json`，只替换 `mcpServers.tianshell`。其他服务器保持原样。项目里的 `.cursor/mcp.json` 不会动。文件如果不是合法 JSON，或者 `mcpServers` 不是对象，就整份不写。

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

只监听环回地址。接口鉴权三者任一即可：

- `Authorization: Bearer <token>`（推荐）
- `x-tianshell-token: <token>`
- 查询参数 `?token=<token>`

工作台里的文件下载和打包下载不走最后一种。界面先用请求头换一张一次性下载票，地址是 `/api/downloads/<票号>`，大约 60 秒、只能用一次。下载地址上如果带了长期令牌，请求会被拒绝，响应里也不写回令牌。

带了 `Origin` 或 `Host` 时，只接受 `localhost` / `127.0.0.1`。

## 给编码工具的用法

TianShell 是会话和记忆，不是一个用完就丢掉的 SSH 客户端。连上一次之后，按这个顺序做事：

1. `list_hosts` 看有哪些机器（别名、地址、端口、用户、标签、备注）。
2. `list_terminal_sessions`。已经有匹配标签就用它；没有就 `open_terminal`。
3. `read_terminal` 看用户眼前的屏幕、cwd、上次退出码。
4. `write_terminal` 或 `run_command` 把命令打进**同一个**标签。用户在 TianShell 窗口里看得到。
5. 命令要跑很久时用 `start_command`，再用 `poll_command` / `cancel_command`，不要卡在一次阻塞调用上。
6. 要读文件时用 `sftp_read` / `sftp_download`。这些调用另开一条 SSH，不会把按键堵在交互终端后面。
7. `search_history` 查这台机器上以前跑过什么。`list_snippets` 然后 `run_snippet` 复用用户存好的命令。

`host` 可以是主机 id 或别名，不区分大小写。本机终端用 `local`。密码、私钥、口令和 MCP 令牌不会出现在工具结果里。

命令会真实执行。只把 TianShell 接到你信任的助手上。

### 会话

| 工具 | 作用 |
| --- | --- |
| `list_terminal_sessions` | 已打开的交互标签：`sessionId`、`kind`（`ssh` 或 `local`）、别名、主机、端口、用户、`hostId`、`cwd`、`lastExit`、是否正忙。没附着工作台时 `attached` 为 false |
| `open_terminal` | `host` 或 `kind: "local"`。已有唯一匹配标签就返回它。`new_session: true` 才再开一个 |
| `close_terminal` | `session_id` |
| `read_terminal` | `session_id` 或 `host`，可选 `max_chars`（默认 12000）。返回 `text`、`cwd`、`lastExit` |
| `write_terminal` | `data` 是按键。`submit: true` 时补上回车，用户看得到 |
| `interrupt_terminal` | 发送 Ctrl+C |

多个标签匹配同一个主机时，工具会列出 `session_id` 并停下来，不会偷偷再登录一次。

### 执行

`run_command` 优先打进已经打开的匹配标签，并等待结果。

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| `command` | 是 | 最长 8000 字符，不能含空字符 |
| `host` | 否 | 主机 id、别名，或 `local`。有 `session_id` 时可以不传 |
| `session_id` | 否 | 指定标签 |
| `timeout_seconds` | 否 | 默认 30，范围 1–120 |
| `new_session` | 否 | true 时不复用已打开的标签。SSH 会另开一条连接；本机会再开一个本地终端 |
| `wait` | 否 | false 时立刻返回 `jobId`，效果和 `start_command` 一样 |

打进标签时，返回 `attached: true`、`stdout`（屏幕上新出现的文本）、`stderr` 为空、`streams: "merged"`、`exitCode`、`cwd`、`jobId`。退出码来自命令后面的一小段收尾：POSIX 用 `$?`，Windows 本机 PowerShell 用 `$LASTEXITCODE`。`fish` 和 `csh` 读不懂这段收尾，这时 `exitCode` 为空，输出仍在 `stdout`。

没有匹配标签时，SSH 的 `run_command` 才另开一条连接，返回分开的 `stdout` / `stderr` 和 `attached: false`。本机没有标签时会先开一个本地终端，让人在窗口里看见。

`start_command`、`poll_command`、`cancel_command` 只针对已经打开的标签。`poll_command` 传 `job_id`。`cancel_command` 发送 Ctrl+C。没有打开的标签时，长任务请先 `open_terminal`，不要指望隐藏的后台 exec。

`session_status` 只保留这次进程里「另开连接」的执行记录，不含界面标签。持久记录用 `search_history`。

### 文件

`sftp_list`、`sftp_mkdir`、`sftp_read`、`sftp_write`、`sftp_download`、`sftp_upload` 都另开一条 SSH。它们不进入交互终端那条 libssh2 线程。工作台标签里的目录刷新、传输和监控走该标签的第二条连接，同样不进终端线程。MCP 的文件工具这次仍各自登录，不改成复用标签上的连接。

| 工具 | 参数 | 限制 |
| --- | --- | --- |
| `sftp_list` | `host`，可选 `path`（默认 `/`） | 列目录。符号链接的 `permissions` 仍是链接本身；`mode` 是它指向的文件或目录的权限位。断链时 `mode` 仍是链接上的 `0777` |
| `sftp_mkdir` | `host`，`path` | 拒绝空路径和 `/` |
| `sftp_read` | `host`，`path`，可选 `max_bytes` | UTF-8，默认 131072，最大 262144。含 NUL、不是 UTF-8，或是私钥内容时会拒绝，不会返回替换字符 |
| `sftp_write` | `host`，`path`，`content` | 覆盖 UTF-8，上限 512KB |
| `sftp_download` | `host`，`path`，可选 `max_bytes` | 返回 base64，最大 256KB。拒绝私钥 |
| `sftp_upload` | `host`，`path`，`content_base64` | 解码后上限 512KB。拒绝私钥。响应里不回显内容 |

`system_metrics` 也是另开一条连接做一次采样（CPU、内存、Swap、磁盘、网卡、Top 进程），CPU 和网卡速率仍用两次采样夹大约 200ms。界面标签上的监控走该标签的第二条 SSH：每次只跑一遍探针，用上一拍的差值算 CPU 和网卡速率，不在终端线程上等待。

### 清单，不含机密

| 工具 | 作用 |
| --- | --- |
| `list_hosts` | 别名、地址、端口、用户、认证方式、分组、跳板、标签、备注。没有密码、私钥、口令 |
| `list_groups` | 分组 |
| `list_snippets` / `save_snippet` / `run_snippet` | 快捷命令。返回值里秘密样式的参数会变成 `[redacted]`。`save_snippet` 拒绝私钥、密码和令牌。`run_snippet` 用保存的原文执行，但结果里的命令文本是抹过的 |
| `list_tunnels` / `start_tunnel` / `stop_tunnel` | 已保存的端口转发。启停只在附着到工作台时可用，并且使用转发自己的连接，不占用交互终端 |
| `get_settings` | 主题、字号、采样间隔。没有令牌 |
| `set_host_note` | 写给人和编码工具看的备注：端口、服务名、注意事项。`notes: null` 清空。拒绝私钥、密码和令牌 |
| `list_bindings` / `save_binding` / `delete_binding` | 本地项目路径 ↔ 主机 + 远端目录。这是记忆，不是自动同步 |

新建带密码或私钥的主机仍在工作台界面里完成，避免密钥在 MCP 结果里走一圈。

### 记忆

`search_history` 按 `query`、`host` 过滤，`limit` 默认 50，最近的在前。记录在数据目录的 `command-history.json`，重启还在。每条有时间、主机 id/别名、会话种类、命令、cwd、退出码或成败、来源（交互输入、MCP、片段）。

交互终端里，TianShell 把送到回车的那一行记下来。Windows ConPTY 上，只发生在控制台内部的行编辑（方向键翻历史）不会被还原。MCP 的 `run_command` 会在知道退出码之后把同一条记录补全。

明显的秘密不会写入历史：`--password` / `--token` 这类参数、`AWS_SECRET_ACCESS_KEY=` 这类赋值、`Bearer`、URL 里的用户名密码、PEM 私钥，以及本机 MCP 令牌。

## 和界面的关系

- 编码工具打进已打开标签的命令，会出现在用户正在看的终端里。
- 顶部「运维」里的命令历史可以浏览这些记录。点一条会把命令填进当前终端，不会直接回车执行。左下角「连接 MCP」仍可复制 Cursor / Claude Code 配置，令牌默认遮住。
- 启动日志只打印界面和 MCP 地址，不打印令牌。
- 一个 SSH 标签在界面上仍是一个会话。终端独占一条 SSH 连接；工作台里的 SFTP 和监控共用另一条不另开标签的连接。关掉标签时两条一起拆掉。
- 不要同时跑两个工作台去写同一个 `TIANSHELL_HOME`。

## For MCP client authors

TianShell is the session and memory hub. Connect once, then drive the open tab.

```bash
cargo run --bin tianshell -- serve
```

Preferred transport: Streamable HTTP `POST/GET/DELETE http://127.0.0.1:53921/mcp`. Spec revisions `2025-03-26`, `2025-06-18`, and `2024-11-05`. Legacy SSE is `GET /sse` plus `POST /message?sessionId=…`.

Auth: `Authorization: Bearer <token>`. The token is stable for a data directory (`TIANSHELL_TOKEN`, otherwise `mcp.token`). The same value is in `runtime.json` while the process is up. Bind address is loopback only. `initialize` returns `Mcp-Session-Id`. Startup logs do not print the token.

Cursor config is `url` plus `headers`, with no `type` field. Claude Code config must set `"type": "http"` (alias `"streamable-http"`). A Claude Code entry that has `url` and no `type` is skipped. Copy-paste JSON is in the sections above; replace `YOUR_TOKEN` or read it from the workbench **连接 MCP** dialog. Do not commit the token.

stdio binary: `tianshell-mcp --attach` proxies to the running workbench. Without `--attach`, stdio can read the same hosts, snippets, and command history, but it cannot see or type into interactive tabs, and it cannot start the workbench's port forwards.

Typical loop: `list_hosts` → `list_terminal_sessions` or `open_terminal` → `read_terminal` → `run_command` (or `write_terminal`) → `sftp_read` → `search_history`. `run_command` types into a matching open tab unless `new_session` is true or no tab matches. On a tab, stdout and stderr are merged and `exitCode` comes from a POSIX `$?` trailer (PowerShell `$LASTEXITCODE` for the local Windows console). `fish` and `csh` do not parse that trailer.

`start_command`, `poll_command`, and `cancel_command` run on an open tab so a long job does not block the client. An interactive tab keeps the shell on its own SSH session. The workbench SFTP pane and the metrics probe use a second silent login, so a listing, a transfer, or a probe does not block typing. MCP file tools (`sftp_list`, `sftp_read`, `sftp_write`, `sftp_download`, `sftp_upload`, `sftp_mkdir`) still open their own SSH login. Private keys, passwords, passphrases, and the MCP token are omitted from tool results and from `command-history.json`.

Interactive history stores the line TianShell submitted. On Windows ConPTY, line editing that happens only inside the console host is not reconstructed. Host notes (`set_host_note`) and project bindings (`list_bindings`) are shared memory for ports, service names, and which local directory belongs on which remote path. Creating a host that needs a password or private key stays in the workbench UI.
