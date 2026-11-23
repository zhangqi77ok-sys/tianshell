/**
 * 终端与 SFTP 左右分屏状态及主机联动逻辑
 */

export interface HostSessionInfo {
  sessionId: string;
  kind: 'ssh' | 'local';
  alias: string;
  username?: string;
  host?: string;
  port?: number;
  closed: boolean;
}

export interface SftpHostState {
  canSftp: boolean;
  status: 'active' | 'local_unsupported' | 'closed' | 'no_session';
  badgeText: string;
  emptyMessage: string;
}

/**
 * 计算当前选中会话在 SFTP 视图中的状态与联动信息
 */
export function resolveSftpHostState(session: HostSessionInfo | null | undefined): SftpHostState {
  if (!session) {
    return {
      canSftp: false,
      status: 'no_session',
      badgeText: '未选中主机',
      emptyMessage: '没有打开的主机会话。双击左侧主机连接后，这里将同步呈现远程文件。',
    };
  }

  if (session.kind === 'local') {
    return {
      canSftp: false,
      status: 'local_unsupported',
      badgeText: '本机终端 (不支持 SFTP)',
      emptyMessage: '当前为本机终端。SFTP 文件管理仅支持远程 SSH 主机，请在上方切换至 SSH 主机标签。',
    };
  }

  if (session.closed) {
    const name = session.alias || (session.host ? `${session.username ?? ''}@${session.host}` : '远程主机');
    return {
      canSftp: false,
      status: 'closed',
      badgeText: `${name} (已断开)`,
      emptyMessage: `主机「${name}」连接已断开。请在上方标签点击重新连接后再操作文件。`,
    };
  }

  const hostDetail = session.host ? ` (${session.username ? `${session.username}@` : ''}${session.host})` : '';
  const label = `${session.alias || '远程主机'}${hostDetail}`;
  return {
    canSftp: true,
    status: 'active',
    badgeText: label,
    emptyMessage: '',
  };
}

export interface SplitLayoutState {
  sftpSideSplit: boolean;
  currentView: 'terminal' | 'sftp' | 'monitor';
}

export interface SplitVisibility {
  isSplitActive: boolean;
  showTerminal: boolean;
  showSftp: boolean;
  showMonitor: boolean;
  menuActionText: string;
}

/**
 * 根据当前分屏配置与当前导航视图，计算各窗格的可见性与右键菜单项文本
 */
export function resolveSplitVisibility(state: SplitLayoutState): SplitVisibility {
  const { sftpSideSplit, currentView } = state;

  if (currentView === 'monitor') {
    // 监控视图全屏呈现，分屏置于后台
    return {
      isSplitActive: sftpSideSplit,
      showTerminal: false,
      showSftp: false,
      showMonitor: true,
      menuActionText: sftpSideSplit ? '关闭右侧文件分屏' : '在右侧分屏打开文件',
    };
  }

  if (currentView === 'sftp') {
    // 用户主动点击顶部「文件」Tab，全屏呈现 SFTP
    return {
      isSplitActive: sftpSideSplit,
      showTerminal: false,
      showSftp: true,
      showMonitor: false,
      menuActionText: sftpSideSplit ? '关闭右侧文件分屏' : '在右侧分屏打开文件',
    };
  }

  // currentView === 'terminal'
  if (sftpSideSplit) {
    // 左右分屏协同模式：左边终端，右边文件
    return {
      isSplitActive: true,
      showTerminal: true,
      showSftp: true,
      showMonitor: false,
      menuActionText: '关闭右侧文件分屏',
    };
  }

  // 纯终端全屏模式
  return {
    isSplitActive: false,
    showTerminal: true,
    showSftp: false,
    showMonitor: false,
    menuActionText: '在右侧分屏打开文件',
  };
}
