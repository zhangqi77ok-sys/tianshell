import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js';
import * as z from 'zod/v4';
import { resolveHostChain, type HostChain } from '../core/host-hops.js';
import type { HostStore } from '../core/host-store.js';
import type { SessionRegistry } from '../core/sessions.js';
import type { ServerMetrics, SftpEntry } from '../shared/types.js';
import {
  connectWithJumps,
  explainSshError,
  type ExecResult,
  type JumpHop,
  type SshTarget,
} from '../ssh/client.js';
import { sampleServerMetrics } from '../ssh/monitor.js';
import { listSftpDir, readRemoteText, writeRemoteText } from '../ssh/sftp.js';

export interface McpRemoteOps {
  listDir: (chain: HostChain, dirPath: string) => Promise<SftpEntry[]>;
  readText: (chain: HostChain, filePath: string, maxBytes: number) => Promise<{ content: string; truncated: boolean }>;
  writeText: (chain: HostChain, filePath: string, content: string) => Promise<void>;
  sampleMetrics: (chain: HostChain) => Promise<ServerMetrics>;
}

export interface ToolDeps {
  hosts: HostStore;
  sessions: SessionRegistry;
  execCommand: (
    target: SshTarget,
    command: string,
    timeoutMs: number,
    acceptHostKey: (fingerprint: string) => boolean,
    jumps?: JumpHop[],
  ) => Promise<ExecResult>;
  opsOverride?: Partial<McpRemoteOps>;
}

const DEFAULT_REMOTE_OPS: McpRemoteOps = {
  listDir: async (chain, dirPath) => {
    const conn = await connectWithJumps(chain.hops);
    try {
      return await listSftpDir(conn.client, dirPath);
    } finally {
      conn.dispose();
    }
  },
  readText: async (chain, filePath, maxBytes) => {
    const conn = await connectWithJumps(chain.hops);
    try {
      return await readRemoteText(conn.client, filePath, maxBytes);
    } finally {
      conn.dispose();
    }
  },
  writeText: async (chain, filePath, content) => {
    const conn = await connectWithJumps(chain.hops);
    try {
      await writeRemoteText(conn.client, filePath, content);
    } finally {
      conn.dispose();
    }
  },
  sampleMetrics: async (chain) => {
    const conn = await connectWithJumps(chain.hops);
    try {
      const { metrics } = await sampleServerMetrics(conn.client);
      return metrics;
    } finally {
      conn.dispose();
    }
  },
};

export function createMcpServer(deps: ToolDeps): McpServer {
  const remoteOps: McpRemoteOps = { ...DEFAULT_REMOTE_OPS, ...(deps.opsOverride ?? {}) };
  const server = new McpServer(
    { name: 'tianshell', version: '0.2.0' },
    {
      instructions:
        'TianShell 保存了用户的 SSH 主机（支持分组和跳板机）。先用 list_hosts 查看主机别名，再调用 run_command / sftp_list / sftp_read / sftp_write / system_metrics 管理服务器。不要猜测密码。',
    },
  );

  server.registerTool(
    'list_hosts',
    {
      title: '列出主机',
      description: '列出本机保存的 SSH 主机（含分组与跳板机关联，不含密码私钥）。List saved SSH hosts without secrets.',
      inputSchema: {},
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
    },
    async () => text(JSON.stringify(deps.hosts.list(), null, 2)),
  );

  server.registerTool(
    'run_command',
    {
      title: '执行命令',
      description: '连接主机（支持 ProxyJump 跳板链）执行命令并返回输出与退出码。Run one command on a saved host.',
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
      description: '查看当前 MCP 进程里的命令会话状态。Read exec session status for this MCP process.',
      inputSchema: { session_id: z.string().optional().describe('可选会话 ID') },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
    },
    async ({ session_id }) => sessionStatus(deps.sessions, session_id),
  );

  server.registerTool(
    'sftp_list',
    {
      title: '列出远端目录',
      description: '通过 SFTP 列出目标主机指定目录的文件与权限。List directory contents over SFTP.',
      inputSchema: { host: z.string().describe('主机 id 或别名'), path: z.string().default('/').describe('POSIX 远端路径') },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: true },
    },
    async ({ host, path: dirPath }) =>
      withResolvedChain(deps.hosts, host, async (chain) => text(JSON.stringify(await remoteOps.listDir(chain, dirPath), null, 2))),
  );

  server.registerTool(
    'sftp_read',
    {
      title: '读取远端文本文件',
      description: '通过 SFTP 只读读取配置/日志等 UTF-8 文本文件（上限 256KB）。Read a remote UTF-8 text file.',
      inputSchema: {
        host: z.string().describe('主机 id 或别名'),
        path: z.string().describe('远端文件路径'),
        max_bytes: z.number().int().min(256).max(262_144).optional().describe('最大读取字节数，默认 131072'),
      },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: true },
    },
    async ({ host, path: filePath, max_bytes }) =>
      withResolvedChain(deps.hosts, host, async (chain) =>
        text(JSON.stringify(await remoteOps.readText(chain, filePath, max_bytes ?? 131_072), null, 2)),
      ),
  );

  server.registerTool(
    'sftp_write',
    {
      title: '写入远端文本文件',
      description: '通过 SFTP 将 UTF-8 文本内容写入远端文件（慎用，会覆盖原文件）。Write UTF-8 text file over SFTP.',
      inputSchema: {
        host: z.string().describe('主机 id 或别名'),
        path: z.string().describe('远端文件路径'),
        content: z.string().describe('完整文件内容（上限 512KB）'),
      },
      annotations: { readOnlyHint: false, destructiveHint: true, openWorldHint: true },
    },
    async ({ host, path: filePath, content }) => {
      if (Buffer.byteLength(content, 'utf8') > 524_288) {
        return text('内容超过 512KB 上限，请改用本地传输上传大文件。', true);
      }
      return withResolvedChain(deps.hosts, host, async (chain) => {
        await remoteOps.writeText(chain, filePath, content);
        return text(`已成功写入 ${filePath}`);
      });
    },
  );

  server.registerTool(
    'system_metrics',
    {
      title: '系统实时监控',
      description: '采集目标主机 CPU、内存、Swap、磁盘挂载和 Top 进程状态。Sample server system metrics.',
      inputSchema: { host: z.string().describe('主机 id 或别名') },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: true },
    },
    async ({ host }) =>
      withResolvedChain(deps.hosts, host, async (chain) => text(JSON.stringify(await remoteOps.sampleMetrics(chain), null, 2))),
  );

  return server;
}

export async function startStdioServer(deps: ToolDeps): Promise<void> {
  const server = createMcpServer(deps);
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

async function withResolvedChain(
  hosts: HostStore,
  hostName: string,
  fn: (chain: HostChain) => Promise<CallToolResult>,
): Promise<CallToolResult> {
  let chain: HostChain | null = null;
  try {
    chain = resolveHostChain(hosts, hostName);
  } catch (err) {
    return text(explainSshError(err), true);
  }
  if (!chain) {
    return text(`没有找到主机「${hostName.trim()}」。可以先调用 list_hosts 查看已保存的别名。`, true);
  }
  try {
    return await fn(chain);
  } catch (err) {
    return text(explainSshError(err), true);
  }
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

  let chain: HostChain | null = null;
  try {
    chain = resolveHostChain(deps.hosts, hostName);
  } catch (err) {
    return text(explainSshError(err), true);
  }
  if (!chain) {
    return text(`没有找到主机「${hostName.trim()}」。可以先调用 list_hosts 看看已保存的别名。`, true);
  }

  const resolved = chain.target;
  const leafHop = chain.hops[chain.hops.length - 1];
  const jumps = chain.hops.slice(0, -1);
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
    const result = await deps.execCommand(leafHop.target, command, timeoutMs, leafHop.acceptHostKey, jumps);
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
    deps.sessions.mark(session.id, { status: 'error', endedAt: new Date().toISOString(), error: message });
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
