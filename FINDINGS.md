# FINDINGS: 文字入力バグ調査結果

**日付**: 2026-03-30
**調査方式**: Delegate Mode（5仮説エージェント並行調査 + 外部AI相談）
**ステータス**: コンセンサス形成済み

---

## エグゼクティブサマリー

ZWG Terminalの文字入力不具合について、5つの独立した仮説エージェントが並行調査を実施。
**7件のバグ**を特定し、うち3件が**Critical**（入力消失を直接引き起こす）。

複数エージェントが独立に同じバグを発見しており、信頼性は高い。

---

## 発見バグ一覧（優先度順）

### S1: Critical（入力消失の直接原因）

#### BUG-1: `close_settings_panel` がTerminalPaneにフォーカスを復元しない
- **発見**: Agent 3 (RootView横取り)
- **ファイル**: `crates/zwg-app/src/app.rs` — `close_settings_panel` (L1327-1333)
- **影響**: 設定パネルをXボタンで閉じた後、キーボード入力がターミナルに到達しない
- **再現手順**:
  1. Ctrl+, で設定パネルを開く
  2. Xボタンで閉じる（ターミナルをクリックしない）
  3. キーボードで文字入力 → **何も入力されない**
  4. ターミナルをクリック → 復活
- **根本原因**: `on_new_tab`, `on_close_pane` 等は `focus_active_terminal(window, cx)` を呼ぶが、`close_settings_panel` はこれを行わない
- **修正案**: `close_settings_panel` に `window` 引数を追加し、末尾で `self.focus_active_terminal(window, cx)` を呼ぶ

#### BUG-2: `GetForegroundWindow()` で他アプリのIMEコンテキストを誤取得
- **発見**: Agent 4 (IME状態不整合), Agent 1 (IME二重経路) — **独立検証済み**
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` — `terminal_input_method_native_mode_active()` (L590-625)
- **影響**: Alt+Tab後に英語入力が消失
- **再現手順**:
  1. ZWG Terminalで日本語IMEオン
  2. Alt+Tab で他アプリへ切り替え（IMEオンのまま）
  3. Alt+Tab で ZWG Terminal に戻る
  4. 英語キーを入力 → **一部のキーがドロップされる**
- **根本原因**: `GetForegroundWindow()` がAlt+Tab直後に前のアプリのHWNDを返し、そのアプリのIMEコンテキストで判定してしまう
- **修正案**: `GetForegroundWindow()` を自プロセスのHWND（または `GetWindowThreadProcessId` で検証）に置換

#### BUG-3: 破棄された入力が `recent_user_inputs` に記録され、後続の正当な入力を連鎖的に誤破棄
- **発見**: Agent 2 (重複検出過剰), Agent 5 (PTY書き込み障害) — **独立検証済み**
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` — `should_drop_duplicate_user_input` (L1862-1911)
- **影響**: IMEで同じ日本語文字を連続入力すると2文字目が消失
- **再現手順**:
  1. 日本語IMEで「ああ」と入力（同じ文字を250ms以内に2回確定）
  2. → 「あ」1文字しか入力されない
- **根本原因**: L1899-1900で `duplicate=true` でも `push_back` して履歴に記録。破棄された `ImeEndComposition` エントリが残り、次の `TextCommit` と CROSS_ROUTE (250ms) で衝突
- **修正案**: `duplicate=true` の場合は `push_back` をスキップ:
  ```
  if !duplicate { self.recent_user_inputs.push_back(...); }
  ```

---

### S2: High（入力ロストの間接原因）

#### BUG-4: `should_route_keystroke_via_text_input` が `IME_VK_PROCESSKEY` を消費しない
- **発見**: Agent 4 (IME状態不整合)
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` (L686-692, 2128-2133)
- **影響**: フラグ残留により、次回キーイベントで英語入力がIME処理中と誤判定
- **修正案**: `load(Acquire)` を `swap(false, AcqRel)` に変更、または `on_key_down` 内で `should_route_keystroke_via_text_input` が true を返した直後に `store(false)` を呼ぶ

#### BUG-5: `write_input()` のエラーをサイレント無視
- **発見**: Agent 5 (PTY書き込み障害)
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` — `write_terminal_bytes` (L1848-1849)
- **影響**: ConPTYパイプ書き込み失敗時にユーザーへのフィードバックなし
- **修正案**: `let _ =` を除去し、エラー時にログ出力 + TerminalState遷移

#### BUG-6: フォーカスイン/アウトでIME状態がリセットされない
- **発見**: Agent 4 (IME状態不整合)
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` — TerminalPane全体
- **影響**: フォーカス復帰時に古いIMEフラグが残留し入力誤判定
- **修正案**: `on_focus_in` で `IME_VK_PROCESSKEY.store(false)`, `ime_composing = false`, キュークリア

---

### S3: Medium（エッジケース・防御的改善）

#### BUG-7: プロセス終了後も `TerminalState::Running` のまま
- **発見**: Agent 5 (PTY書き込み障害)
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` (L930-933)
- **影響**: シェル終了後にキー入力しても壊れたパイプに書き続ける
- **修正案**: `ProcessExited` イベント時に新しい `Exited` ステートに遷移

#### BUG-8: Pending状態で重複検出をバイパス
- **発見**: Agent 5 (PTY書き込み障害)
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` — `on_key_down` (L2162-2164)
- **影響**: Pending中はIME二重送信を検出できず、二重入力のリスク
- **修正案**: `write_terminal_bytes` 直呼びを `write_user_input_bytes` 経由に変更

#### BUG-9: `should_route_keystroke_via_text_input` 後の `stop_propagation` 欠如
- **発見**: Agent 1 (IME二重経路), Agent 3 (RootView横取り) — **独立検証済み**
- **ファイル**: `crates/zwg-app/src/terminal/view.rs` (L2128-2133)
- **影響**: gpuiの合成TranslateMessageが二重に呼ばれる可能性
- **修正案**: `return` 前に `cx.stop_propagation()` を追加（ただし `flush_ime_endcomposition_queue` 経路の確実な動作を検証後）

---

## 棄却された仮説

| 仮説 | 棄却理由 |
|------|---------|
| ASCII入力がIME text_input経路に迷い込む | Agent 1 がコードレベルで3重ガード（processkey/native_mode/非ASCII）の健全性を証明 |
| 通常時にRootViewがキーを横取り | Agent 3 がgpuiバブリング順序（子→親）を確認。TerminalPaneにフォーカスがある限り安全 |

---

## 推奨修正順序

```
Phase 1（即座に修正 — 入力消失の直接解消）:
  ├── BUG-1: close_settings_panel のフォーカス復元
  ├── BUG-3: duplicate detection の push_back 条件修正
  └── BUG-4: IME_VK_PROCESSKEY の swap/store 修正

Phase 2（Alt+Tab/フォーカス関連）:
  ├── BUG-2: GetForegroundWindow → 自プロセスHWND
  └── BUG-6: フォーカスイン/アウトのIME状態リセット

Phase 3（防御的改善）:
  ├── BUG-5: write_input エラーハンドリング
  ├── BUG-7: ProcessExited 時のstate遷移
  ├── BUG-8: Pending状態の重複検出一貫性
  └── BUG-9: stop_propagation 追加
```

---

## 外部AI (Cipher: Grok/Gemini/ChatGPT) のコンセンサス

- IME_VK_PROCESSKEY残留 → 既知パターン、タイムアウト付きフラグへの変更を推奨
- 250ms重複検出ウィンドウ → 150ms以下に短縮しても安全
- ConPTY stdin書き込みは非同期キュー方式への移行を長期的に推奨
- Windows IME + ConPTY の組み合わせはknown difficult area。徹底的なログ取得が鍵

---

## 調査に参加したエージェント

| エージェント | 仮説 | 所要時間 | ツール使用回数 |
|------------|------|---------|-------------|
| hypothesis-1-ime-dual-route | IME二重経路 | ~9分 | 80 |
| hypothesis-2-duplicate-detection | 重複検出過剰 | ~3分 | 36 |
| hypothesis-3-rootview-intercept | RootView横取り | ~4.5分 | 67 |
| hypothesis-4-ime-state-inconsistency | IME状態不整合 | ~4.3分 | 52 |
| hypothesis-5-pty-write-failure | PTY書き込み障害 | ~2分 | 29 |
