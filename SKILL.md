# SKILL.md - Claude Code スキル定義

スラッシュコマンドで呼び出せるスキル（ワークフロー）の定義。
/fast は project-local command（`.claude/commands/fast.md`）として提供する。
/codex委任ルールの詳細はCLAUDE.mdを参照。

---

## 個別GPTエキスパートSkills（Codex CLI版）

> **Zenn記事アプローチ：MCPの可視性問題を解決**
> **出典**: https://zenn.dev/owayo/articles/63d325934ba0de

### MCP vs Skill アプローチの違い

| 項目 | MCP (mcp__codex__codex) | Skill (codex exec) |
|------|--------------------------|---------------------|
| **進捗表示** | なし（ブラックボックス） | リアルタイム表示 |
| **実行時間** | 10分〜1時間待ち | 同じだが進捗が見える |
| **中断** | 不可 | 可能（Ctrl+C） |
| **エラー表示** | 見えない | ターミナルに表示 |
| **デバッグ** | 困難 | 容易 |

### 6つの個別Skills

| Skill | スラッシュコマンド | 説明 |
|-------|-------------------|------|
| **codex** | `/codex` | 汎用コードレビュー・分析 |
| **architect** | `/architect` | システム設計・アーキテクチャ決定 |
| **plan-reviewer** | `/plan-reviewer` | 実装計画の検証 |
| **code-reviewer** | `/code-reviewer` | コード品質・バグ発見 |
| **security-analyst** | `/security-analyst` | 脆弱性診断・脅威モデリング |
| **scope-analyst** | `/scope-analyst` | 要件分析・スコープ明確化 |

### 使用方法

#### スラッシュコマンド（推奨）

```bash
# システム設計相談
/architect

# コードレビュー依頼
/code-reviewer

# セキュリティチェック
/security-analyst
```

#### 手動実行

```bash
# 基本形式
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_dir> "<request>"

# 例：コードレビュー
codex exec --model gpt-5.2-codex --sandbox read-only --cd /path/to/project "このコードをレビューして、改善点を指摘してください"
```

### 各Skillの詳細

#### 1. `/codex` - 汎用コードレビュー

```bash
codex exec --full-auto --sandbox read-only --cd <project_directory> "<request>"
```

**使用場面**:
- コードベース全体の分析
- バグの調査
- リファクタリング提案
- 解消が難しい問題の調査

#### 2. `/architect` - システム設計

```bash
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_directory> "<design_request>"
```

**使用場面**:
- システム設計決定
- アーキテクチャ選択
- トレードオフ分析
- 技術選定

**7セクションフォーマット推奨**

#### 3. `/plan-reviewer` - 計画検証

```bash
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_directory> "<plan_review_request>"
```

**使用場面**:
- 実装計画のレビュー
- タスク分解の検証
- 依存関係の確認
- 曖昧性の特定

#### 4. `/code-reviewer` - コード品質

```bash
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_directory> "<code_review_request>"
```

**使用場面**:
- コード品質チェック
- バグ発見
- ベストプラクティス検証
- パフォーマンス改善提案

#### 5. `/security-analyst` - セキュリティ

```bash
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_directory> "<security_check_request>"
```

**使用場面**:
- 脆弱性診断
- OWASP Top 10チェック
- 脅威モデリング
- CVSS スコアリング

#### 6. `/scope-analyst` - 要件分析

```bash
codex exec --model gpt-5.2-codex --sandbox read-only --cd <project_directory> "<scope_analysis_request>"
```

**使用場面**:
- 曖昧な要件の明確化
- スコープ境界の定義
- リスク特定
- 追加質問の生成

### 実行例

```bash
# システム設計の相談
/architect
# プロンプト: "このWebアプリケーションのデータベース設計について、スケーラビリティとコストのトレードオフを分析してください"

# 計画のレビュー
/plan-reviewer
# プロンプト: "この実装計画に不足している項目や依存関係の問題がないか検証してください"

# コードレビュー
/code-reviewer
# プロンプト: "認証処理のコードをレビューして、セキュリティ上の問題を指摘してください"
```

---

---

## X Research スキル（Grok x_search）

> **X(Twitter)リアルタイム検索・トレンド分析専用**

### スキル概要

| 項目 | 内容 |
|------|------|
| **スキル名** | x-research / x-trend-ideas |
| **目的** | X投稿のリアルタイム検索、トレンド分析、投稿ネタ発見 |
| **API** | xAI API (Grok x_search) |
| **特徴** | 他のAIにはないX投稿のリアルタイム検索能力 |

### スラッシュコマンド

| コマンド | 用途 |
|---------|------|
| `/x-research` | 記事執筆前の周辺リサーチ（一次情報/用語/反論/数字を収集） |
| `/x-trend-ideas` | X投稿ネタ出し（impressions最大化、トレンド探索） |

### 手動実行

```bash
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "トピック" --audience both --days 7
```

### 自動発動条件

リサーチ・情報収集タスクでは WebSearch と**並行して自動実行**される。

### 使用場面

| 場面 | 説明 |
|------|------|
| **X投稿トレンド調査** | 直近のバズ投稿、論点クラスターを抽出 |
| **記事ネタリサーチ** | 一次情報/用語/反論/数字を揃えるContext Pack作成 |
| **投稿ネタ出し** | impressions最大化のための素材生成 |
| **技術トレンド収集** | AI/Web3/開発者ツール文脈のリアルタイム情報 |

---

## スキル発動条件

| スキル | 発動条件 | 優先度 |
|--------|----------|--------|
| **/codex (Codex CLI)** | 設計・計画・レビュー・セキュリティ | Tier 0（最優先） |
| **個別GPTエキスパートSkills** | 同上（可視性が必要な場合） | Tier 1（推奨） |
| **x-research** | X投稿検索・トレンド分析（リサーチ時自動並行実行） | 有効 |
| **python-performance** | .py ファイル編集時 | 該当時必須 |
| **go-development** | .go ファイル編集時 | 該当時必須 |
| **language-selection** | 新規プロジェクト作成時 | 該当時必須 |
