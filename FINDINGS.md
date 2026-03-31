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


---

# FINDINGS #2: ESCキー問題調査結果

**日付**: 2026-03-30
**調査方式**: Delegate Mode（5仮説エージェント並行調査 + マルチAI議論: Gemini/Grok/OpenAI/Perplexity）
**ステータス**: コンセンサス形成済み

---

## エグゼクティブサマリー

ZWG Terminal内でClaude Codeを実行すると、ESCキーが機能しない問題。
5つの独立エージェントが並行調査し、**全員が同一の根本原因を特定**。
4つの外部AIによる議論でコンセンサスを形成済み。

**根本原因**:  がESCキー（key_char=None）をIME処理中と誤判定し、PTYへの送信をブロックしている。

---

## 根本原因の詳細

### BUG-10 (Critical):  がESCを含む制御キーをIME deferする

- **発見**: Agent 1〜5 全員が独立に特定（**最高信頼度**）
- **ファイル**:  —  (L690-732)
- **確信度**: 5/5（全エージェント・全AI一致）

#### メカニズム



#### 問題のコード（L731-732）



### 副次原因: app-level on_key_down がESCをstop_propagation

- **ファイル**:  —  (L2555-2590)
- gpuiはparent-firstのイベント伝播
-  の場合、L2572で  が呼ばれ、ターミナルペインにESCが到達しない

---

## 5エージェント調査結果

| Agent | 仮説 | 確信度 | 結論 |
|-------|------|--------|------|
| agent-1-ime-target | root_ime_target がESCをブロック | 3/5 | 部分支持（AI設定パネル時のみ） |
| agent-2-ime-composing | IME composing がESCを飲み込み | 4/5 | **強く支持** |
| agent-3-dispatch-order | gpui dispatch順序がESCを遮断 | 4/5 | **支持**（parent-first確認） |
| agent-4-ime-flag | IME_VK_PROCESSKEY がESCを無視 | 4/5 | **強く支持（最有力）** |
| agent-5-hotkeys-palette | global_hotkeys/paletteが横取り | 3.5/5 | 反証→IME原因に収束 |

### エージェント間の相互反証

- Agent-1の「root_ime_target」仮説: Agent-3,5が「AI設定パネル不使用時には成立しない」と反証→部分支持に格下げ
- Agent-5の「global_hotkeys」仮説: 自身の調査でglobal_hotkeysにESC未登録を確認→自己反証
- Agent-2,4の「IME defer」仮説: 全エージェントが独立に到達→最高信頼度

---

## マルチAI議論結果

### 修正案

| 案 | 内容 | Gemini | Grok | OpenAI | Perplexity |
|----|------|--------|------|--------|------------|
| A | 制御キー白リスト化 | ○ | ○ | △ | ◎推奨 |
| B | key_char=None一律非defer | △ | △ | ◎推奨 | △ |
| C | app-level ESCバイパス | △ | △ | × | × |
| D | A+C 両層修正 | ◎推奨 | ◎推奨 | △ | △ |

### コンセンサス

- **全AI一致**:  の修正が最優先
- **案Aベース + 必要に応じてapp-level修正**: Gemini/Grok/Perplexityの3社が支持
- **案Bの筋の良さ**: OpenAIが「IMEにdeferすべきは文字入力のみ」と主張

### 採用方針: 案A（制御キー白リスト）を主修正 + app-level監視

**理由**:
1. 最小変更で最大効果
2. Enter/Backspace/TabはIME確定に使われるため、案Bの一律除外はリスクあり
3. ESC/矢印キー等はIME処理と無関係で安全に除外可能
4. app-level (案C)は状況監視し、必要時に追加

---

## 推奨修正

### Phase 1: 即座に修正（ESC問題解消）

#### 修正箇所1:  — 



#### 修正箇所2（任意）:  — 

ESCをリストから除外する検討:


### Phase 2: 検証

1.  でログ取得し、ESCが正しくPTYに送信されることを確認
2. 日本語IME有効状態でClaude CodeのESCが機能することを確認
3. IME変換中の他キー（Enter確定、矢印候補移動等）が正常動作することを確認

---

## 棄却された仮説

| 仮説 | 棄却理由 |
|------|---------|
| global_hotkeysがESCを横取り | Agent-5がglobal_hotkeysにESC未登録を確認 |
| snippet_paletteがESCを消費 | show_snippet_palette=false時は早期リターン |
| root_ime_targetがstuck | AI設定パネル未使用時はNone |

---

## 調査に参加したエージェント

| エージェント | 仮説 | 確信度 |
|------------|------|--------|
| agent-1-ime-target | root_ime_target ブロック | 3/5 |
| agent-2-ime-composing | IME composing 飲み込み | 4/5 |
| agent-3-dispatch-order | gpui dispatch順序 遮断 | 4/5 |
| agent-4-ime-flag | IME_VK_PROCESSKEY stuck | 4/5 |
| agent-5-hotkeys-palette | global_hotkeys/palette 横取り | 3.5/5 |

## 外部AI参加

| AI | 推奨案 | 主な主張 |
|----|--------|---------|
| Gemini | 案D (A+C) | 両層修正が最も堅牢 |
| Grok | 案D (A+C) | 多層問題には多層修正 |
| OpenAI | 案B | key_char=Noneは制御キー、defer不要 |
| Perplexity | 案A | 白リスト化が最小変更で低リスク |

---

# FINDINGS #3: IME日本語入力 二重登録バグ

**日付**: 2026-03-31
**調査方式**: Delegate Mode（5仮説エージェント並行調査）
**ステータス**: コンセンサス形成済み

---

## エグゼクティブサマリー

Claude Code上で日本語入力すると、テキストが二重登録される問題。
**例**: 「日本語入力をすると」→「日本語入力をすると日本語入力をすると」

5つの独立したエージェントが調査し、**根本原因を特定**。

---

## 調査体制

| Agent | 仮説 | 結果 |
|-------|------|------|
| 1: dual-hook | 二重WH_GETMESSAGEフック競合 | **補強**: gpui composing分岐の二重TranslateMessage発見 |
| 2: queue | IME_COMPOSITION_RESULT_QUEUE二重配信 | **根本原因確認** |
| 3: keydown | on_key_downパスからのIMEテキスト漏洩 | **副次原因確認**: IME_VK_PROCESSKEYライフサイクル問題 |
| 4: rootview | RootView/TerminalPane InputHandler競合 | **否定**: フォーカスハンドルが別のため競合なし |
| 5: timing | 重複検出タイミング不整合 | **補強**: SAME_ROUTE 30msウィンドウが不十分 |

---

## 根本原因

### テキストがPTYに到達する3つの経路

```
┌─────────────────────────────────────────────────────────────┐
│  WM_IME_COMPOSITION (GCS_RESULTSTR)                        │
│                                                             │
│  ┌──── WH_GETMESSAGE Hook (view.rs) ──────┐                │
│  │  queue_ime_endcomposition_text()        │                │
│  │  → IME_COMPOSITION_RESULT_QUEUE に追加  │ ──経路A──→ PTY │
│  └─────────────────────────────────────────┘                │
│                                                             │
│  ┌──── gpui WndProc (events.rs L685) ─────┐                │
│  │  handle_ime_composition()               │                │
│  │  → replace_text_in_range()              │ ──経路B──→ PTY │
│  └─────────────────────────────────────────┘                │
│                                                             │
│  WM_IME_ENDCOMPOSITION                                     │
│  ┌──── WH_GETMESSAGE Hook ───────────────┐                 │
│  │  queue_ime_endcomposition_text()       │ ──経路C──→ PTY  │
│  │  (同じテキストを再度キューに追加)       │                 │
│  └────────────────────────────────────────┘                 │
└─────────────────────────────────────────────────────────────┘
```

**同一の確定テキストが最大3回PTYに書き込まれる構造。**

### 重複検出メカニズムの限界

| パラメータ | 値 | 用途 |
|-----------|-----|------|
| CROSS_ROUTE_DUPLICATE_WINDOW_MS | 250ms | 異なるソース間(ImeEndComposition vs TextCommit) |
| SAME_ROUTE_COMMIT_DUPLICATE_WINDOW_MS | **30ms** | 同一ソース間(ImeEndComposition vs ImeEndComposition) |

**問題**: 経路Aと経路Cは同一ソース(ImeEndComposition)だが、経路Cのフラッシュは次の`render_running`で行われるため、30ms以上経過することがある（30fps以下で確実に超過）。

### 二重登録の発生フロー

```
[T=0ms]   Hook: WM_IME_COMPOSITION → キューに"あ" (経路A)
[T=0ms]   gpui: replace_text_in_range("あ")
           └→ flush: キューから"あ"取り出し → write(ImeEndComposition, "あ") ✓記録
           └→ write(TextCommit, "あ") → クロスルート重複検出(250ms) → ✓DROP
[T=1ms]   Hook: WM_IME_ENDCOMPOSITION → キューに"あ" (経路C)
[T=33ms]  render_running → flush: キューから"あ"取り出し
           └→ write(ImeEndComposition, "あ") → 同一ルート重複検出(30ms)
           └→ elapsed=33ms > 30ms → ✗検出失敗 → "あ"が二重書き込み!
```

---

## 修正計画

### Fix 1: WM_IME_COMPOSITIONからのキュー追加を削除 (根本修正)

**ファイル**: `crates/zwg-app/src/terminal/view.rs` L428-436

gpui 0.2.2が同じ`WM_IME_COMPOSITION`メッセージから`replace_text_in_range`で確実にテキストを配信するため、フックからのキュー追加は冗長。削除することで経路Aを排除。

`WM_IME_ENDCOMPOSITION`(経路C)はgpuiが配信に失敗した場合のセーフティネットとして維持。

### Fix 2: SAME_ROUTE_COMMIT_DUPLICATE_WINDOW_MSを30ms→100msに増加

30fps(33ms)でも`WM_IME_ENDCOMPOSITION`重複を確実に検出できるようにする。

### 修正後のフロー

```
[T=0ms]   gpui: replace_text_in_range("あ")
           └→ flush: キュー空 → スキップ
           └→ write(TextCommit, "あ") ✓記録
[T=1ms]   Hook: WM_IME_ENDCOMPOSITION → キューに"あ" (セーフティネット)
[T=16ms]  render_running → flush: キューから"あ"取り出し
           └→ write(ImeEndComposition, "あ") → クロスルート重複検出(250ms)
           └→ elapsed=16ms < 250ms → ✓DROP (正常)
```

---

## 否定された仮説

### RootView/TerminalPane InputHandler競合 (Agent 4)

- RootViewの`replace_text_in_range`はAI設定テキストフィールド専用
- ターミナル入力中はTerminalPaneがフォーカスを持つ
- gpuiはフォーカスされたハンドルのInputHandlerにのみIMEイベントを配信
- **競合は発生しない**

---

## 関連ファイル

| ファイル | 関連箇所 |
|---------|---------|
| `crates/zwg-app/src/terminal/view.rs` | L193(IME_VK_PROCESSKEY), L212(queue), L403(hook), L2130(dedup), L2680(replace_text_in_range) |
| `crates/zwg-app/src/app.rs` | L79(INPUT_METHOD_VK_PROCESSKEY), L215(root hook), L5394(RootView replace_text) |
| gpui-0.2.2 `events.rs` | L372(handle_keydown_msg), L657(handle_ime_composition), L685(GCS_RESULTSTR) |
