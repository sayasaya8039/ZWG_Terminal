# codex-app-server MCP Bridge

Claude Code から `codex app-server --listen stdio://` を呼び出すためのプロジェクトローカル MCP サーバーです。

## Tools

- `codex_chat`: 新しい Codex thread を作成して 1 turn 実行します。
- `codex_continue`: 既存 thread に prompt を追加します。
- `codex_thread_read`: thread の内容を読み取ります。
- `codex_auth_status`: token を返さずに Codex 認証状態を確認します。
- `codex_model_list`: app-server が返すモデル一覧を取得します。

## Configuration

リポジトリルートの `.mcp.json` に `codex-app-server` を追加します。Claude Code を再起動すると MCP tool として読み込まれます。

```json
{
  "mcpServers": {
    "codex-app-server": {
      "type": "stdio",
      "command": "node",
      "args": ["mcp/codex-app-server/server.mjs"]
    }
  }
}
```

既定では以下を起動します。

```bash
codex app-server --listen stdio://
```

必要な場合は `.mcp.json` の `env` で上書きできます。

```json
{
  "CODEX_APP_SERVER_COMMAND": "codex",
  "CODEX_APP_SERVER_ARGS": "[\"app-server\", \"--listen\", \"stdio://\"]",
  "CODEX_APP_SERVER_DEFAULT_CWD": "D:\\NEXTCLOUD\\Windows_app\\ZWG_Terminal"
}
```

`CODEX_APP_SERVER_ARGS` は JSON 配列、または空白区切りの文字列を受け付けます。

Windows では npm 版 Codex の `bin/codex.js` が見つかる場合、`node codex.js app-server --listen stdio://` で起動します。見つからない場合は `codex` コマンドにフォールバックします。

tool 引数で `cwd` を指定しない場合、Codex thread の作業ディレクトリはこの MCP パッケージから見たリポジトリルートに解決されます。必要なら `CODEX_APP_SERVER_DEFAULT_CWD` で上書きできます。

## Safety

既定の `codex_chat` は `approvalPolicy: "never"` と `sandbox: "read-only"` で thread を開始します。app-server から approval request が届いた場合、このブリッジは安全側に倒して拒否します。書き込みを許可したい場合は tool 引数で `sandbox: "workspace-write"` などを明示してください。

## Verification

```bash
npm run check
npm test
npm run smoke
```

`npm test` は MCP プロトコルの初期化と tool 一覧を検証します。`npm run smoke` はローカルの `codex app-server` を実際に起動して `codex_auth_status` まで確認します。
