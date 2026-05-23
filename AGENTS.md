# AGENTS.md

## Repository Rules

- Beadsを使用してください。
- 設定ファイルに修正、更新などを行った場合は必ずバージョンを上げてください。
- どのような場合でも、ユーザー対応は必ず日本語で行ってください。
- 作業は最後まで一気に仕上げてください。作業終了時には GitHub にコミットし、プッシュし、最後にビルドまで行ってください。
- 作業はできるだけ Agent Teams を組んで行ってください。
- 通知リングシステムは最重要機能として扱い、関連不具合の検出・修正を最優先してください。

## フルスタックコーディング

- Planner→Generator→Evaluatorの順で開発を進める。
- Plannerはプロンプトを受け取り、それを完全な製品仕様書に展開する。詳細な技術実装ではなく、製品のコンテキストと高レベルの技術設計に焦点を当てる。UIは限りなくMacOS風に近づけるように設計。製品仕様書にAI機能を組み込む機会を見つけるよう。
- Generatorは一度に1つの機能を追加するアプローチ。Generatorはスプリント単位で作業。仕様書から一度に1つの機能を取得する。各スプリントでは、zig、wasm、gpui、julia、NPU、MultiCPU、React、Vite、FastAPI、SQLite（PostgreSQL）のスタックを使用してアプリを実装。各スプリントの最後にQAに引き渡す前に、自身の作業を自己評価する。バージョン管理にはGitを使用。構築する内容と成功の検証方法を提案。
- Evaluatorはユーザーが行うように実行中のアプリケーションをクリックし、UI機能、APIエンドポイント、データベースの状態をテスト。発見したバグと、フロントエンド実験をモデルにした一連の基準（ここでは製品の深度、機能性、ビジュアルデザイン、コード品質を網羅するように調整）に基づいて、各スプリントを評価。ジェネレーターが正しいものを構築していることを確認し、両者は合意に至るまで議論を繰り返す。

## 成長プロセス（Memento-Skills 式自己進化 $2014 必須）

> **全エージェントに適用。タスク完了後のReflect-Writeフェーズは省略禁止。**

### サイクル

```
Read（既存スキル検索）→ Execute（実行）→ Reflect（構造抽出）→ Write（スキル記録）
```

### エージェント別の責務

| エージェント | Read Phase | Reflect-Write Phase |
|-------------|-----------|-------------------|
| **Planner** | 過去の設計パターンを検索 | 計画の成功/失敗構造を記録 |
| **Generator** | 実装パターン（gpui, FFI等）を検索・適用 | 新規パターンを原子スキルとして記録 |
| **Evaluator** | 過去のバグパターンを検索 | 発見したバグの構造的原因を記録 |

### 蓄積済みS評価パターン（即適用）

| ID | パターン | utility_score |
|----|---------|--------------|
| 002 | 4体Agent Teams並行修正（司令塔+担当+品質） | 0.85 |
| 003 | gpui SharedString lifetime 所有化 | 0.90 |
| 005 | アトミック設定更新（依存フィールド一括） | 0.90 |
| 007/011 | 競合OSS分析→原則抽出→適応移植 | 0.90 |
| 008 | 5仮説エージェント並行調査+クロス検証 | 0.85 |
| 009 | AttachConsole パイプ破壊回避 | 0.85 |

### 複合バグ調査プロトコル（Delegate Mode）

1. リードがコードを読み、5つの独立仮説を設計
2. 5体のExploreエージェントに各仮説を割り当て
3. 各エージェントが独立調査（共有コンテキストなし）
4. リードがクロス検証（2+エージェント収束 → 高信頼）
5. リード自身が核心部分を直接検証（エージェント報告を鵜呑みにしない）

### スキルライブラリ

`~/.claude/skills/learned/SKILL.md` $2014 12パターン蓄積済み。タスク開始時に `tags[]` で検索し、`utility_score $2265 0.8` は即座に適用する。

<!-- BEGIN BEADS INTEGRATION v:1 profile:full hash:f65d5d33 -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-powered version control with native sync
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
bd update <id> --claim --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task atomically**: `bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Quality
- Use `--acceptance` and `--design` fields when creating issues
- Use `--validate` to check description completeness

### Lifecycle
- `bd defer <id>` / `bd supersede <id>` for issue management
- `bd stale` / `bd orphans` / `bd lint` for hygiene
- `bd human <id>` to flag for human decisions
- `bd formula list` / `bd mol pour <name>` for structured workflows

### Auto-Sync

bd automatically syncs via Dolt:

- Each write auto-commits to Dolt history
- Use `bd dolt push`/`bd dolt pull` for remote sync
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- END BEADS INTEGRATION -->
