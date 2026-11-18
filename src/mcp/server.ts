import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js';
import * as z from 'zod/v4';
import type { HostStore } from '../core/host-store.js';
import type { SessionRegistry } from '../core/sessions.js';
import { explainSshError, type ExecResult, type SshTarget } from '../ssh/client.js';

export interface ToolDeps {
  hosts: HostStore;
  sessions: SessionRegistry;
  execCommand: (
    target: SshTarget,
    command: string,
    timeoutMs: number,
    acceptHostKey: (fingerprint: string) => boolean,
  ) => Promise<ExecResult>;
}

const OUTPUT_NOTE = '命令会在远程机器上真实执行。凭据来自本机已保存的主机，不会在结果里返回。';

export function createMcpServer(deps: ToolDeps): McpServer {
  const server = new McpServer(
    { name: 'tianshell', version: '0.1.0' },
    {
      instructions:
        'TianShell 保存了用户的 SSH 主机。先用 list_hosts 查看别名，再用 run_command 执行命令。不要猜测密码。session_status 只反映这个 MCP 进程里的命令，看不到桌面里打开的交互终端。',
    },
  );

  server.registerTool(
    'list_hosts',
    {
      title: '列出主机',
      description: '列出本机保存的 SSH 主机，不包含密码或私钥。List saved SSH hosts without secrets.',
      inputSchema: {},
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
    },
    async () => text(JSON.stringify(deps.hosts.list(), null, 2)),
  );

  server.registerTool(
    'run_command',
    {
      title: '执行命令',
      description: `用已保存的凭据连接主机并执行一条命令，返回输出和退出码。${OUTPUT_NOTE} Run one command on a saved host.`,
      inputSchema: {
        host: z.string().describe('主机 id 或别名'),
        command: z.string().describe('要执行的命令'),
        timeout_seconds: z.number().int().min(1).max(120).optional().describe('超时秒数，默认 30，最大 120'),
      },
      annotations: { readOnlyHint: false, destructiveHint: true, openWorldHint: true },
    },
    async ({ host, command, timeout_seconds }) => runCommand(deps, host, command, timeout_seconds),
  );

  server.registerTool(
    'session_status',
    {
      title: '会话状态',
      description:
        '查看当前 MCP 进程里的命令会话。不含桌面终端里正在交互的那个会话。Read exec session status for this MCP process.',
      inputSchema: {
        session_id: z.string().optional().describe('不填则返回最近的会话列表'),
      },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
    },
    async ({ session_id }) => sessionStatus(deps.sessions, session_id),
  );

  return server;
}

export async function startStdioServer(deps: ToolDeps): Promise<void> {
  const server = createMcpServer(deps);
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

async function runCommand(
  deps: ToolDeps,
  hostName: string,
  rawCommand: string,
  timeoutSeconds?: number,
): Promise<CallToolResult> {
  const command = rawCommand.trim();
  if (!command) return text('命令是空的', true);
  if (command.length > 8000) return text('命令太长了（上限 8000 个字符）', true);
  if (command.includes('\0')) return text('命令里不能有空字符', true);

  const resolved = deps.hosts.resolve(hostName);
  if (!resolved) {
    return text(`没有找到主机「${hostName.trim()}」。可以先调用 list_hosts 看看已保存的别名。`, true);
  }

  const timeoutMs = (timeoutSeconds ?? 30) * 1000;
  const session = deps.sessions.open({
    hostId: resolved.id,
    alias: resolved.alias,
    host: resolved.host,
    kind: 'exec',
    status: 'running',
    command,
  });

  try {
    const result = await deps.execCommand(
      {
        host: resolved.host,
        port: resolved.port,
        username: resolved.username,
        authType: resolved.authType,
        secret: resolved.secret,
        passphrase: resolved.passphrase,
      },
      command,
      timeoutMs,
      (fingerprint) => deps.hosts.rememberFingerprint(resolved.id, fingerprint),
    );
    deps.sessions.mark(session.id, {
      status: 'closed',
      endedAt: new Date().toISOString(),
      exitCode: result.exitCode,
      timedOut: result.timedOut,
    });
    return text(
      JSON.stringify(
        {
          sessionId: session.id,
          alias: resolved.alias,
          host: resolved.host,
          port: resolved.port,
          username: resolved.username,
          command,
          exitCode: result.exitCode,
          timedOut: result.timedOut,
          truncated: result.truncated,
          stdout: result.stdout,
          stderr: result.stderr,
        },
        null,
        2,
      ),
    );
  } catch (error) {
    const message = explainSshError(error);
    deps.sessions.mark(session.id, {
      status: 'error',
      endedAt: new Date().toISOString(),
      error: message,
    });
    return text(message, true);
  }
}

function sessionStatus(sessions: SessionRegistry, sessionId?: string): CallToolResult {
  if (!sessionId) return text(JSON.stringify(sessions.list(), null, 2));
  const found = sessions.get(sessionId);
  if (!found) return text(`没有编号为 ${sessionId} 的会话`, true);
  return text(JSON.stringify(found, null, 2));
}

function text(body: string, isError = false): CallToolResult {
  return { isError, content: [{ type: 'text', text: body }] };
}
