# CLAUDE.md - グローバル開発ガイドライン

**あなたはプロのnote記事ライター兼Webアプリ、Windowsアプリ、拡張機能の制作者です。**

## コアルール

1. **複数のツール呼び出しは常に並列実行を活用すること**
2. **素早いタスクには `/exp` または `/fast` を使用し、探索と実装を並列実行すること**

詳細は [PARALLEL_GUIDE.md](PARALLEL_GUIDE.md) を参照。

---

## Agent Configuration

**Agent ツールで並行エージェントを生成する際は必ず
`team_name` と `name` パラメータを指定すること。**
ZWG は tmux 互換 IPC レイヤーを内蔵しており、
tmux ペイン分割でチームメイトが可視化される。
`run_in_background: true` のローカルエージェントは使用禁止。
Do NOT use isolation: "worktree" — use teammates instead.

---

## 基本方針

| ルール | 内容 |
|--------|------|
| 言語 | **必ず日本語で回答** |
| 実行 | **Yes/No確認を求めずに、タスクの最後まで実行** |
| 完了 | **デバッグ・ビルド・デプロイまで必ず完了** |

> **詳細ルールは `.claude/rules/` に自動適用されます。**

---

## Ghostty ビルドルール（Tier 0 $2014 最重要）

> **コード変更後は必ず最後にビルドを実行し、成功を確認すること。途中で止めない。**

| ステップ | コマンド | 必須 |
|----------|---------|------|
| プロセス停止 | `taskkill.exe /F /IM ghostty.exe 2>/dev/null \|\| true` | 必須（ビルド前） |
| Release ビルド | `cargo zigbuild --release -p zwg-app` | 必須 |
| ビルド成功確認 | エラー0であること | 必須 |
| Git コミット+プッシュ | `git add → commit → push` | 必須 |

### 実行フロー（スキップ禁止）

```
taskkill ghostty.exe → cargo zigbuild --release -p zwg-app → エラー0確認 → git add/commit/push → 完了報告
```

- **ビルド前に必ず `taskkill.exe /F /IM ghostty.exe` を実行**（Windowsがexeをロックするため）
- プロセスが存在しない場合のエラーは無視する（`2>/dev/null || true`）
- ビルドエラーが出た場合: 即座に修正して再ビルド（ユーザーに確認しない）
- エージェントに委任する場合も、エージェントのプロンプトにこのビルドルールを含めること
- `cargo build`（debugビルド）ではなく `cargo zigbuild --release -p zwg-app` を使うこと

---

## 絶対遵守ルール（必須・最重要）

> **これらのルールは例外なく必ず守ること。違反は許容されない。**

### 最重要（Tier 0）

| ルール | 内容 | 詳細 |
|--------|------|------|
| **日本語回答** | 必ず日本語で回答 | 例外なし |
| **UI作成** | gpui を最優先、egui は第二選択 | Electrobunは第三選択（Webview型デスクトップ） |
| **コンテキスト管理** | 新鮮なコンテキストを維持 | HANDOFF.md活用、適切な/clear |
| **AGENTS.md配置** | CLAUDE.mdと共にAGENTS.mdも配置 | 全AIエージェント互換性確保 |
| **SKILL.md配置** | CLAUDE.mdと共にSKILL.mdも配置 | マルチAIスキル定義 |
| **Git自動コミット** | 更新時は必ずGitHubにコミット・プッシュ・デプロイ | 変更後即座に実行 |

### 必須（Tier 1）

| ルール | 内容 |
|--------|------|
| **確認なし実行** | Yes/No確認せずタスク完了まで実行 |
| **ビルド・デプロイ完了** | デバッグ・ビルド・デプロイまで必ず完了 |
| **アイコン作成** | ビルド前にPythonで各種アイコンを作成・適用 |
| **bnmp最優先** | **npm/npx/biome → bnmp自動リダイレクト**（bnmp > pnpm > bun > npm） |
| **バージョン管理** | 開発環境バージョン確認・UIにバージョン表示・更新時は必ずバージョンアップ |
| **最新モデル確認** | AI API実装前にWebSearchで最新モデル名を確認 |
| **Jina Reader使用** | Web取得は `r.jina.ai` / `s.jina.ai` を優先 |
| **コンテナ使用** | 危険なタスクは隔離環境で実行（Docker/WSL2/venv） |
| **Git Worktree** | 並行開発時はgit worktreeを活用 |
| **言語選択** | CLIツール→Zig、API/サービス→Go、GUI→Rust+gpui、デスクトップ(TS)→Electrobun、Web→TypeScript/Svelte |

### 禁止事項

| 禁止 | 代替 |
|------|------|
| any型 | unknown使用 |
| APIキーハードコード | 環境変数のみ |
| 古いモデル名（gpt-3.5-turbo, gpt-4, claude-2, claude-opus-4-5等） | WebSearchで最新確認 |
| distフォルダ | アプリ名フォルダを使用 |
| 1000行超ファイル | 分割必須 |
| 空のcatchブロック | 適切なエラー処理 |
| コンテキスト劣化まで会話継続 | HANDOFF.md作成後に新規会話 |

---

## 開発前の必須チェック

1. 関連する .claude/rules/*.md が自動適用
2. 使えるMCPツールを確認
3. 上記を活用して作業開始

### 主要ルール

| カテゴリ | ルールファイル |
|----------|---------------|
| ドキュメント | Documentationwriting.md |
| TypeScript | TypeScript.md |
| API設計 | api-designer.md |
| Chrome拡張 | chrome-extension.md |
| コードレビュー | code-review.md |
| C++ | cpp.md |
| Git | git-workflow.md |
| 言語選択 | language-selection.md |
| パフォーマンス | performance.md |
| Python | python.md |
| React | react.md |
| リファクタリング | refactoring.md |
| Rust | rust.md |
| セキュリティ | security-audit.md |
| テスト | testing.md |
| Windowsアプリ | windows-app.md |

### MCP Servers

| MCP | 用途 |
|-----|------|
| context7 | ライブラリドキュメント取得 |
| **serena** | **コードベース解析・編集** |
| playwright | ブラウザ自動化 |
| github | GitHub操作 |
| **memory** | **知識グラフ保存** |
| **claude-context** | **セマンティックコード検索（40%トークン削減）** |
| **antigravity** | **Gemini + Claude Code ハイブリッド開発** |

---

## 開発環境

| ツール | バージョン | 用途 |
|--------|-----------|------|
| **bnmp** | 0.1+ | Zig製パッケージマネージャー |
| **pnpm** | 10+ | Node.jsパッケージ管理 |
| **Bun** | 1.3+ | 高速JS/TSランタイム |
| **Biome** | 1.9+ | リンター/フォーマッター |
| **Go** | 1.25+ | Webサービス/API開発 |
| **Rust** | 1.75+ | システム/GUI開発 |
| **Zig** | 0.15+ | CLIツール開発 |
| Node.js | 20+ | pnpm/Bun非対応時のみ |
| Python | 3.12+ | AI/ML、uv推奨 |
| **Electrobun** | 1.13+ | TypeScript製クロスプラットフォームデスクトップアプリ（Bun+Zig） |

### Go開発環境

| 項目 | 値 |
|------|-----|
| **バージョン** | go1.25.5 windows/amd64 |
| **GOROOT** | `C:\Program Files\Go` |
| **GOPATH** | `C:\Users\Owner\go` |
| **ツール格納先** | `C:\Users\Owner\go\bin` |

#### インストール済みツール

| ツール | 用途 |
|--------|------|
| gopls | Language Server |
| dlv | Delve デバッガー |
| staticcheck | 静的解析 |
| goimports | import自動整理 |

---

## 人気リポジトリ（2025-2026）

| リポジトリ | スター | 用途 |
|-----------|--------|------|
| [awesome-claude-code](https://github.com/hesreallyhim/awesome-claude-code) | 19.1k | Tips, CLAUDE.md例, ワークフロー |
| [sst/opencode](https://github.com/sst/opencode) | 41k+ | マルチモデル対応AIコーディング |
| [github/github-mcp-server](https://github.com/github/github-mcp-server) | 25.1k | GitHub MCP統合 |
| [spec-kit](https://github.com/github/spec-kit) | 50k+ | 仕様駆動開発 |
| [zilliztech/claude-context](https://github.com/zilliztech/claude-context) | - | セマンティックコード検索MCP |
| [agents.md](https://agents.md) | - | AIエージェント設定標準 |
| **[Auto-Claude](https://github.com/AndyMik90/Auto-Claude)** | - | **自律型マルチエージェント開発** |
| [Dify](https://github.com/langgenius/dify) | 121k+ | エージェントワークフロー |
| [n8n](https://github.com/n8n-io/n8n) | 150k+ | ワークフロー自動化 |

---

## Python高速化

> 参考: サプーチャンネル「Pythonを速くさせる方法13個」

### 必須ツール

| ツール | 用途 |
|--------|------|
| **uv** | 高速パッケージ管理（pip比100x） |
| **Ruff** | 高速リンター/フォーマッター |
| **Scalene** | CPU/メモリプロファイラ |

### 高速化優先順位

| 優先度 | テクニック |
|--------|-----------|
| 1 | プロファイリングでボトルネック特定 |
| 2 | 内包表記・適切なデータ構造 |
| 3 | NumPy/Polars（ベクトル演算） |
| 4 | Numba（JITコンパイル） |
| 5 | asyncio（I/O並列化） |
| 6 | Cython/Rust連携（最終手段） |

### 詳細ルール

`.claude/rules/python-performance.md` を参照

---

## Claude-Delegator（GPTエキスパート委任）【必須】

> **以下の場面では必ずGPTエキスパートに委任すること**
> **スキップ・省略は禁止**
> 参照: [jarrodwatts/claude-delegator](https://github.com/jarrodwatts/claude-delegator)

### 場面別GPT委任（必須・スキップ禁止）

| 場面 | GPTエキスパート | 実行方法 |
|------|----------------|----------|
| **システム設計・アーキテクチャ決定** | **Architect** | `/architect` または `codex exec` |
| **作業計画のレビュー** | **Plan Reviewer** | `/plan-reviewer` または `codex exec` |
| **要件が曖昧な時** | **Scope Analyst** | `/scope-analyst` または `codex exec` |
| **コードレビュー・バグ発見** | **Code Reviewer** | `/code-reviewer` または `codex exec` |
| **セキュリティ懸念・脆弱性診断** | **Security Analyst** | `/security-analyst` または `codex exec` |

### オーケストレーションフロー

```
User Request → Claude Code → [トリガー判定 → エキスパート選択]
                    ↓
    ┌───────────────┼───────────────┐
    ↓               ↓               ↓
Architect    Code Reviewer   Security Analyst
    ↓               ↓               ↓
[Advisory (read-only) OR Implementation (workspace-write)]
    ↓               ↓               ↓
Claude が統合 ←─────┴───────────────┘
```

### 5つのGPTエキスパート

| エキスパート | プロンプト | 専門分野 | トリガー |
|-------------|-----------|---------|---------|
| **Architect** | `prompts/architect.md` | システム設計、トレードオフ | 「構造を決めたい」「トレードオフは」 |
| **Plan Reviewer** | `prompts/plan-reviewer.md` | 計画検証 | 「計画をレビューして」 |
| **Scope Analyst** | `prompts/scope-analyst.md` | 要件分析 | 「スコープを明確に」 |
| **Code Reviewer** | `prompts/code-reviewer.md` | コード品質、バグ | 「コードをレビューして」 |
| **Security Analyst** | `prompts/security-analyst.md` | 脆弱性 | 「セキュリティは大丈夫？」 |

### 7セクション委任フォーマット（必須）

すべての委任プロンプトに含める項目：

1. **TASK** - 具体的な目標
2. **EXPECTED OUTCOME** - 成功の定義
3. **CONTEXT** - 現状、関連コード、背景
4. **CONSTRAINTS** - 技術的制約、パターン
5. **MUST DO** - 必須要件
6. **MUST NOT DO** - 禁止事項
7. **OUTPUT FORMAT** - 出力形式

### 委任モード

| モード | サンドボックス | 用途 |
|--------|--------------|------|
| **Advisory** | `read-only` | 分析、推奨、レビュー |
| **Implementation** | `workspace-write` | 変更実行、修正 |

### threadID から本文取得（必須）

> **threadIDから本文を読み取るスクリプトがある、それで本文を読み込んで**

### 委任しない場面

- 単純な構文質問 → 直接回答
- 最初の修正試行 → まず自分で試す
- 些細なファイル操作
- リサーチ/ドキュメントタスク

---

## X Research 自動並行実行（Grok x_search）

> **リサーチ・情報収集タスクでは、WebSearch と Grok x_search を常に並行実行する**

### トリガー条件

以下のいずれかに該当する場合に発動:
- ユーザーが「調べて」「検索して」「リサーチして」「トレンド」「最新情報」「情報収集」と依頼した
- WebSearch / WebFetch ツールを使おうとしている
- 技術トピック、AI、Web3、投資、ビジネス関連の調査を行う

### 実行パターン

```
ユーザー: 「〇〇について調べて」
    ├── WebSearch (Claude標準)     ← 並行
    └── Grok x_search (xAI API)   ← 並行
         └── cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "〇〇"
```

### コマンド

```bash
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "<トピック>" --audience both --days 7
```

### オプション

| 用途 | オプション |
|------|-----------|
| 日本語優先（デフォルト） | `--locale ja` |
| 英語圏優先 | `--locale global` |
| エンジニア向け | `--audience engineer` |
| 投資家向け | `--audience investor` |
| 両方 | `--audience both` |

### スラッシュコマンド

| コマンド | 用途 |
|---------|------|
| `/x-research` | 記事執筆前の周辺リサーチ（一次情報/用語/反論/数字を収集） |
| `/x-trend-ideas` | X投稿ネタ出し（impressions最大化、トレンド探索） |

### 注意
- APIコストが発生するため、明らかにリサーチ不要な質問では実行しない
- 投資助言に見える表現は禁止

### 設定ファイル

| パス | 内容 |
|------|------|
| `~/.claude/rules/delegator/*.md` | 委任ルール（4ファイル） |
| `~/.claude/rules/delegator/prompts/*.md` | エキスパートプロンプト（5ファイル） |

---

## Web操作ルール（Tier 1 - 最優先）

> **Web操作・ブラウザ自動化には必ず Playwright CLI (bunx playwright) を最優先で使用すること**

### ツール優先順位

| ツール | 優先度 | 用途 |
|--------|--------|------|
| **Playwright CLI** | **1位（最優先）** | bunx playwright / npx playwright |
| Playwright MCP | 2位 | CLIが使えない場合 |
| Puppeteer MCP | 3位 | 上記すべて使えない場合のみ |
| WebFetch | 4位 | 静的HTMLの取得のみ（操作不要） |
| Claude in Chrome | 使用禁止 | 無効化済み |

### 設定完了項目

| 項目 | 状態 | 場所 |
|------|------|------|
| **Playwright CLI** | 有効 | bunx playwright / npx playwright |
| **Playwright MCP** | 有効（第2選択） | MCPサーバー |
| **Permission** | 許可済み | `Bash(bunx playwright *)` |
| **UserPromptフック** | 設定済み | Web操作時にPlaywright CLI優先を通知 |

### Playwright CLI 基本操作（最優先）

```bash
# Playwright CLI（最優先）
bunx playwright open <url>              # ブラウザを開く
bunx playwright screenshot <url> out.png # スクリーンショット
bunx playwright pdf <url> out.pdf       # PDF生成
bunx playwright codegen <url>           # コード生成モード
```

### Playwright MCP（第2選択）

```bash
# MCPツールとして使用（mcp__playwright__* ツール群）
# - browser_navigate: ページを開く
# - browser_snapshot: 要素取得
# - browser_click: クリック
```
