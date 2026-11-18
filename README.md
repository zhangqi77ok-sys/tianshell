# TianShell

一个带界面的 SSH 客户端，里面嵌了一个 MCP 服务。你可以像平时一样连上服务器敲命令，也可以让支持 MCP 的助手按已保存的主机去执行命令。

作者是 zq（[zhangqi77ok-sys](https://github.com/zhangqi77ok-sys)）。这是一个还在长大的工具，不是功能清单的演示页。

## 现在能做什么

- 添加、修改、删除 SSH 主机（别名、地址、端口、用户名）
- 用密码或 OpenSSH/PEM 私钥登录
- 打开一个交互式终端，用完可以断开
- 凭据用 AES-256-GCM 加密后留在本机
- 通过 MCP 列出主机、在主机上执行一条命令、查看这次 MCP 进程里的会话

文件面板现在只是占位。浏览目录、上传、下载还没做。桌面里同一时间只开一个终端。

## 环境

需要 Node.js 22 或更高版本。Windows 10 及以上可以直接用。macOS 和常见的桌面 Linux 也可以，只要 Electron 能起来。

## 运行

```bash
npm install
npm start
```

`npm start` 会先编译，再打开窗口。改完代码后重新执行一次即可。

第一次连接某台主机时，会记住它的主机密钥指纹。以后指纹变了会拒绝连接。如果是你自己换过密钥，编辑这台主机，勾选「清除已记住的主机指纹」，保存后再连。

## 数据放在哪

默认目录：

- Windows：`%APPDATA%\tianshell`
- macOS：`~/Library/Application Support/tianshell`
- Linux：`~/.config/tianshell`

里面主要是 `hosts.json` 和 `vault.key`。密码和私钥在 `hosts.json` 里是密文。`vault.key` 是解开它们的钥匙。旁边的 `app` 文件夹是 Electron 自己的缓存，删掉不影响已保存的主机。

系统密钥环可用时（Windows 的 DPAPI、macOS 钥匙串、带 libsecret 的 Linux），钥匙会再包一层，只放给当前用户。密钥环不可用时，钥匙文件会以仅当前用户可读的方式写在数据目录里。能读到你整个用户目录的程序，仍然有机会解开它。不要把这两个文件交给别人，也不要提交到 git。

想换目录可以设置 `TIANSHELL_HOME`。桌面程序和 MCP 要指向同一个目录，助手才看得到你保存的主机。

## 测试

```bash
npm test
npm run typecheck
```

测试覆盖加密、主机库存取、MCP 工具，以及在本机临时拉起的 SSH 服务上执行命令和打开终端。不会连接外网。

## MCP

TianShell 的 MCP 走 stdio。先在桌面里把主机加好，再让助手调用工具。

在 Cursor 里可以这样写（路径改成你自己的）：

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

仓库里执行 `node scripts/mcp.mjs` 也一样。它会先编译，再用 Electron 启动 MCP，这样和桌面程序使用同一套钥匙。日志在 stderr，stdout 只留给协议消息。

如果数据目录不是默认位置，加上环境变量：

```json
"env": {
  "TIANSHELL_HOME": "D:\\secrets\\tianshell"
}
```

### 工具

| 工具 | 作用 |
| --- | --- |
| `list_hosts` | 列出已保存的主机，不含密码和私钥 |
| `run_command` | 连上某台主机，执行一条命令，返回输出和退出码 |
| `session_status` | 查看这个 MCP 进程里的命令会话 |

`run_command` 的参数是 `host`（id 或别名）、`command`，以及可选的 `timeout_seconds`（默认 30，最长 120）。命令会在远程机器上真实执行，请只把它接到你信任的助手上。输出太长会被截断。

桌面里打开的交互终端，和 MCP 的命令会话是分开的。MCP 看不到你正在敲的那一个窗口。两边打通留到后面再做。

## 目录

```text
src/core      主机库、加密、会话记录
src/ssh       SSH 连接和命令执行
src/mcp       MCP 工具
src/main      Electron 主进程
src/renderer  界面
test          单元测试
```

## 后面想做的

- SFTP：列目录、上传、下载
- 多标签终端
- 跳板机
- 让 MCP 看见当前桌面会话
- 安装包，先照顾 Windows

## 许可

[MIT](LICENSE)
