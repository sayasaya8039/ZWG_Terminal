# FINDINGS: 同じ文字列が繰り返し表示されるバグ

**日付**: 2026-03-13
**調査手法**: Delegate Mode（5 Agent + 5 External AI Debate）
**対象**: ZWG Terminal v1.0.0 + Ghostty VT Backend
**症状**: Claude Code のステータスバー "Opus 4.6 | $0.00 session..." が3行に表示される

---

## 1. コンセンサス（全10ソース一致）

### 根本原因: `lib.zig` の `Handler.vt()` が重要なVTアクションを黙殺

`D:\NEXTCLOUD\Windows_app\ZWG_Terminal\crates\ghostty-vt-sys\zig\lib.zig` 184行目:
```zig
else => {},
```

Ghostty の stream パーサーが正しくパースした **約70種のVTアクション** のうち、Handler は **17種のみ** を処理し、残り **約50種を `else => {}` でサイレントドロップ** している。

---

## 2. 直接原因の詳細（優先度順）

### CRITICAL-1: Alt Screen Buffer が有効化されない

**合意度: 10/10（全ソース一致）**

現在の実装（lib.zig 151-156行）:
```zig
.set_mode => {
    self.terminal.modes.set(value.mode, true);   // ビットのみ設定
},
.reset_mode => {
    self.terminal.modes.set(value.mode, false);  // ビットのみクリア
},
```

**欠落**: `terminal.switchScreenMode()` が呼ばれない。

正しい実装（stream_readonly.zig 212-231行）:
- mode 47 → `switchScreenMode(.@"47", enabled)`
- mode 1047 → `switchScreenMode(.@"1047", enabled)`
- mode 1049 → `switchScreenMode(.@"1049", enabled)`（カーソル保存 + Alt画面切替 + クリア）
- mode origin → `setCursorPos(1, 1)`

**影響**: Claude Code が `ESC[?1049h` を送信しても Alt Screen に切り替わらない。全TUI出力が Primary Screen のスクロールバック付きバッファに蓄積される。

### CRITICAL-2: DECSTBM（スクロール領域）が設定されない

**合意度: 10/10**

`.top_and_bottom_margin` アクションが `else => {}` で破棄される。`terminal.setTopAndBottomMargin()` が一度も呼ばれない。

**影響**: スクロール領域が常に画面全体のまま。ステータスバー行がスクロールに巻き込まれる。

**3つのコピーが発生するメカニズム**:
```
T1: ステータスバーを行23に描画
T2: LF → 全画面スクロール → 行23→行22に移動、行23空に
T3: ステータスバーを行23に再描画 → 行22と行23に2コピー
T4: LF → 全画面スクロール → 行22→行21、行23→行22
T5: ステータスバーを行23に再描画 → 行21, 行22, 行23に3コピー
```

### CRITICAL-3: scroll_up / scroll_down が動作しない

**合意度: 9/10**

`.scroll_up` → `terminal.scrollUp(count)` が呼ばれない（`!void`を返すメソッド）。
`.scroll_down` → `terminal.scrollDown(count)` が呼ばれない。

**影響**: スクロール領域内のスクロール操作が完全に無視される。

### HIGH-1: save_cursor / restore_cursor が動作しない

**合意度: 10/10**

`.save_cursor` / `.restore_cursor` が `else => {}` で破棄。

`terminal.saveCursor()` は以下を保存:
- カーソル位置 (x, y)
- 文字スタイル (bold, italic, fg/bg色)
- pending_wrap 状態
- origin モード
- 文字セット (G0/G1/G2/G3)

**影響**: ステータスバー更新後にカーソルが元の位置に戻らない。

### HIGH-2: insert_lines / delete_lines が動作しない

**合意度: 9/10**

`.insert_lines` / `.delete_lines` が破棄。ConPTY は CSI L/M を積極的に使用する。

**影響**: 行の挿入・削除が反映されず、古い内容が残存。

### MEDIUM: reverse_index / insert_blanks / delete_chars / erase_chars

**合意度: 8/10**

これらも全て `else => {}` で破棄されている。

---

## 3. エージェント間の議論・反証結果

| エージェント | 仮説 | 結果 | 備考 |
|-------------|------|------|------|
| Agent 1 (VT Handler Gap) | Handler欠落が根因 | **確認** | stream_readonly.zig と比較して50+アクション欠落を特定 |
| Agent 2 (Alt Screen) | switchScreenMode未呼出 | **確認** | screens.active が primary のまま固定されることを証明 |
| Agent 3 (DECSTBM) | スクロール領域欠如 | **確認** | Terminal.zig の index()/linefeed() がscrolling_regionを参照する設計を確認 |
| Agent 4 (Cursor Save/Restore) | カーソル復元不全 | **確認** | saveCursor()が7つの状態を保存することを確認 |
| Agent 5 (ConPTY + Line Ops) | 行操作欠落 + ConPTY増幅 | **確認** | ConPTYがCSI L/Mを多用する事実を確認 |

### 反証された仮説

| 仮説 | 反証理由 |
|------|---------|
| Snapshot dirty追跡のバグ | Agent 2が検証: take_dirty_viewport_rows()は正しく動作（脏フラグを1回のみ返す） |
| dump_viewport_rowのページ境界バグ | Agent 3が検証: 2つのpin()が同一行を指すため通常は問題なし |
| リサイズ時のスナップショット複製 | Agent 5が検証: Vec::resize()は既存要素を複製しない |
| PTYリーダーのVTシーケンス分断 | Agent 5が検証: Ghostty Streamはステートマシンでパーシャルシーケンスを保持 |

---

## 4. 外部AIコンセンサス

| AI | #1原因 | #2原因 | #3原因 |
|----|--------|--------|--------|
| Gemini | Alt Screen | DECSTBM | Save/Restore Cursor |
| Grok | DECSTBM | Alt Screen | Scroll Ops |
| OpenAI | DECSTBM | scroll_up/down | save/restore cursor |
| Perplexity | Alt Screen | DECSTBM | scroll_up/down |
| ZAI | Alt Screen | DECSTBM | insert/delete lines |

**全5つのAIがAlt Screen + DECSTBMを最上位に挙げた。**

---

## 5. 修正計画

### Phase 1: 最小修正（ステータスバー3重表示の解消）

**修正ファイル**: `crates/ghostty-vt-sys/zig/lib.zig` の `Handler.vt()` 関数

追加すべきアクション（4種）:
1. `set_mode` / `reset_mode` に Alt Screen 判定追加 → `switchScreenMode()`
2. `.top_and_bottom_margin` → `setTopAndBottomMargin()`
3. `.save_cursor` → `saveCursor()`
4. `.restore_cursor` → `restoreCursor()`

### Phase 2: スクロール・行操作の実装

追加すべきアクション（8種）:
5. `.scroll_up` → `try scrollUp(value)`
6. `.scroll_down` → `scrollDown(value)`
7. `.reverse_index` → `reverseIndex()`
8. `.insert_lines` → `insertLines(value)`
9. `.delete_lines` → `deleteLines(value)`
10. `.insert_blanks` → `insertBlanks(value)`
11. `.delete_chars` → `deleteChars(value)`
12. `.erase_chars` → `eraseChars(value)`

### Phase 3: 安定性向上

13. `.full_reset` → `fullReset()`
14. `.index` → `try index()`
15. `.next_line` → `try index()` + `carriageReturn()`

### API注意点

| メソッド | 戻り値 | 処理 |
|---------|--------|------|
| `scrollUp(n)` | `!void` | `catch {}` で消費（VT標準では無効操作は無視） |
| `switchScreenMode(mode, enabled)` | `!void` | `catch {}` で消費 |
| `setTopAndBottomMargin(top, bottom)` | `void` | そのまま呼出 |
| `saveCursor()` / `restoreCursor()` | `void` | そのまま呼出 |
| `top_and_bottom_margin` の value | `Margin{top_left, bottom_right}` | 0=デフォルト（top=1, bottom=rows） |

### リファレンス実装

- `vendor/ghostty/src/terminal/stream_readonly.zig` (212-231行): setMode の正しい実装
- `vendor/ghostty/src/termio/stream_handler.zig` (244-310行): 全アクションの処理例

---

## 6. 副次的な改善（Phase 2以降）

| 項目 | 詳細 |
|------|------|
| TERM環境変数 | ConPtyConfig.env に `TERM=xterm-256color` を追加 |
| リサイズ順序 | PTY → backend の順に変更検討 |
| `else => {}` のログ | 未処理アクションをdebugログに出力 |

---

## 7. 結論

**確信度: 98%** — lib.zig の Handler が約50種のVTアクションを `else => {}` で黙殺していることが根本原因。Terminal.zig 側には全メソッドが実装済みであり、Handler の switch 文に対応する呼び出しを追加するだけで修正可能。

修正対象ファイルは **1つだけ**: `crates/ghostty-vt-sys/zig/lib.zig`
