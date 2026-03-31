# HANDOFF.md - セッション引き継ぎ (2026-03-27)

## 完了タスク

### 1. スニペットEDIT編集バグ修正
**問題**: スニペットパレットの編集(EDIT)ボタンが履歴セクションでも表示されていたが、クリックしても何も起きなかった。`open_template_editor_for_edit()` が `SnippetSection::Template` 以外でサイレントリターンしていた。

**修正内容** (`crates/zwg-app/src/app.rs`):
- EDITボタンを `.when(is_template, ...)` でTemplateセクションのみ表示に変更
- 履歴アイテムで Ctrl+E 押下時に「履歴は編集できません」通知を表示
- Ctrl+E キーボードショートカットを `handle_snippet_palette_key` に追加

**コミット**: `9705eef fix: snippet edit button only for templates, add Ctrl+E shortcut`

### 2. 未コミット変更の整理
29ファイルの未コミット変更を7つの論理コミットに整理してプッシュ済み。

| # | コミット | 種類 | 内容 |
|---|---------|------|------|
| 1 | `c7c0669` | chore | ビルド設定・Zigキャッシュ管理の再編 |
| 2 | `1de513f` | feat | GPU差分レンダリング・PTYバーストコアレシング |
| 3 | `7fdf6b8` | refactor | ターミナルレンダリングをGPUネイティブパイプラインに移行 |
| 4 | `108bef7` | refactor | スニペットシステムを単一モジュールに統合（snippets/ディレクトリ4ファイル削除） |
| 5 | `49407c3` | chore | デッドコード・未使用API・FINDINGSドキュメント削除 |
| 6 | `9705eef` | fix | スニペット編集ボタン修正 + Ctrl+E ショートカット + IME状態検出リファクタ |
| 7 | `6a4142b` | chore | PIXキャプチャを.gitignoreに追加 |

## 現在のブランチ状態

- **ブランチ**: `main`
- **リモート**: プッシュ済み (`6a4142b`)
- **ワーキングツリー**: クリーン（未コミット変更なし）
- **ビルド**: `cargo zigbuild --release -p zwg` 成功確認済み（warning 1件: `active_text_mut` dead code）

## プロジェクト構造メモ

- **パッケージ名**: `zwg`（Cargo.toml の name、`cargo zigbuild --release -p zwg`）
- **バイナリ名**: `zwg`（`crates/zwg-app/src/main.rs`）
- **プロセス名**: `ghostty.exe`（taskkill対象）
- **ワークスペース**: `zwg`, `ghostty-vt`, `ghostty-vt-sys`
- **UIフレームワーク**: gpui 0.2

## 残りの既知課題

- `template_editor.rs:213` の `active_text_mut` が未使用（warning）
- スニペットパレットで履歴アイテムの「コピー」ボタンが削除済み（PasteToTerminal に統一）— 意図的な変更かユーザーに確認推奨
