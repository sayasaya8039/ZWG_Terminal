# AGENTS.md

## Repository Rules

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

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

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
