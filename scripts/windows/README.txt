TianShell for Windows
=====================

Double-click tianshell.exe. It starts the workbench on 127.0.0.1:53921 and opens a browser. No cargo flags.

Leave the console window open. Closing it stops the process.

Optional install (current user, no administrator):

    powershell -ExecutionPolicy Bypass -File .\install.ps1

That copies the programs to %LOCALAPPDATA%\TianShell and adds a Start Menu shortcut named TianShell.

MCP is already listening at http://127.0.0.1:53921/mcp. In the workbench, click "连接 MCP" for copy-paste Cursor and Claude Code configs. The token is shown there and is not printed by this installer.

tianshell-mcp.exe is the stdio fallback. With the workbench already running:

    tianshell-mcp.exe --attach

Data defaults to %APPDATA%\tianshell. Set TIANSHELL_HOME only if the workbench and a stdio client must share a different folder.
