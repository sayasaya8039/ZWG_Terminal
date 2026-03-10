# ZWG_Terminal Delegate Mode Review — Findings Document

> **日付**: 2026-03-10
> **レビュアー**: 8名（内部5エージェント + 外部3AI）
> **対象**: ZWG_Terminal v1.0.0 全ソースコード（~2,200行）

## レビュアー一覧

| # | レビュアー | 専門領域 | 仮説 |
|---|-----------|---------|------|
| A1 | Agent (Security) | 並行性・レース条件 | Mutex順序/デッドロック/チャネル問題 |
| A2 | Agent (Code Review) | メモリ・リソースリーク | scrollback超過/Entity保持/Drop不備 |
| A3 | Agent (Code Review) | VTパーサ・レンダリング | UTF-8/カーソルずれ/色処理 |
| A4 | Agent (Architect) | アーキテクチャ・拡張性 | 設定断絶/責務過多/密結合 |
| A5 | Agent (Code Review) | エラー処理・堅牢性 | パニック/ハング/エッジケース |
| G | Grok | 総合（アーキ/並行/リソース） | 外部レビュー |
| Ge | Gemini | 正確性/unsafe/エッジケース | 外部レビュー |
| C | ChatGPT | パフォーマンス/UX/本番品質 | 外部レビュー |

---

## コンセンサス・マトリクス

### Tier 0: CRITICAL（全員一致 — 即時修正必須）

| ID | 問題 | 合意数 | レビュアー | 場所 |
|----|------|--------|-----------|------|
| **C1** | VTパーサのUTF-8デコード欠如 | 3/8 | A3,Ge,C | `vt_parser.rs:104-108` |
| **C2** | CJK全角文字でカーソルが1しか進まない | 5/8 | A3,Ge,C,G,A4 | `mod.rs:184` |
| **C3** | Config/ThemeがUI層に到達しない | 5/8 | A4,C,G,Ge,A3 | `view.rs:8-10`, `app.rs` |
| **C4** | リサイズがペーンサイズでなくウィンドウ全体を使用 | 4/8 | A4,Ge,C,A3 | `view.rs:101-107` |

### Tier 1: HIGH（複数レビュアー合意 — 早期修正推奨）

| ID | 問題 | 合意数 | レビュアー | 場所 |
|----|------|--------|-----------|------|
| **H1** | unboundedチャネルでメモリ爆発 | 4/8 | A1,A2,G,C | `surface.rs:146` |
| **H2** | Drop時のjoin()がUIスレッドをブロック | 4/8 | A1,A2,Ge,G | `surface.rs:232` |
| **H3** | プロセス終了がUIに通知されない | 3/8 | A5,C,A2 | `view.rs:37-60` |
| **H4** | write_charのO(n²)パディング | 3/8 | Ge,C,A3 | `mod.rs:147-153` |
| **H5** | unsafe as_bytes_mut のString破壊リスク | 3/8 | Ge,A5,A3 | `mod.rs:158-160` |
| **H6** | ポーリングループがプロセス終了後も継続 | 3/8 | A5,A2,A1 | `view.rs:37-60` |
| **H7** | ConPTYエラーパスでパイプHANDLEリーク | 2/8 | A5,Ge | `pty.rs:197-207` |
| **H8** | 256色モード(idx 16-255)が未実装 | 2/8 | A3,Ge | `vt_parser.rs:391-396` |

### Tier 2: MEDIUM（要改善 — 次フェーズで対応）

| ID | 問題 | 合意数 | レビュアー | 場所 |
|----|------|--------|-----------|------|
| **M1** | cell_width=8.4のハードコード | 4/8 | A3,Ge,C,A4 | `view.rs:65` |
| **M2** | EventEmitter<()>が情報を伝えない | 3/8 | A4,G,C | `split/mod.rs:342` |
| **M3** | AppState/RootViewの責務過多 | 2/8 | A4,G | `app.rs` |
| **M4** | ghostty_vtのexpect()パニック | 1/8 | A5 | `surface.rs:34` |
| **M5** | resize()でscrollback上限チェックなし | 1/8 | A2 | `mod.rs:213-234` |
| **M6** | OSC文字列のUTF-8処理不備 | 1/8 | A3 | `vt_parser.rs:552` |
| **M7** | u16→i16キャストでCOORDオーバーフロー | 1/8 | A5 | `pty.rs:311-312` |
| **M8** | キーバインドのハードコード | 2/8 | A4,G | `main.rs:17-28` |
| **M9** | unsafe impl Sync for PtyPairの過剰宣言 | 1/8 | A1 | `pty.rs:42` |
| **M10** | F-key/Alt/Ctrl+Shift未対応 | 2/8 | C,A3 | `view.rs:314-343` |

### Tier 3: LOW（改善推奨 — 優先度低）

| ID | 問題 | レビュアー | 場所 |
|----|------|-----------|------|
| L1 | カーソル可視チェック欠如 | A3 | `view.rs:149` |
| L2 | ProcessExitedのexit codeが常に0 | A1 | `surface.rs:201` |
| L3 | config読み込みエラーの通知なし | A5 | `config/mod.rs:162` |
| L4 | タブタイトル番号の重複 | A4 | `app.rs:54-67` |
| L5 | ESC+\のST終端処理不完全 | A3 | `vt_parser.rs:545` |

---

## 詳細分析: Tier 0 CRITICAL

### C1: VTパーサのUTF-8デコード欠如

**全体像**: フォールバックVTパーサ（`vt_parser.rs`）が非ASCII バイト（0x80以上）を個別に処理し、各バイトを `REPLACEMENT_CHARACTER` (U+FFFD) に変換する。UTF-8の3バイト文字「あ」(E3 81 82) は3つの `?` として表示される。

**コンセンサス**:
- A3: "日本語環境では実質的に使用不能"
- Ge: "string slicing will panic on multi-byte boundaries"
- C: "logically wrong for terminal cells"

**影響度**: ghostty_vt フィーチャー OFF 時に全ての非ASCII文字が破壊される。日本語ターミナルとして致命的。

**修正方針**: VtParser にUTF-8ステートマシンを追加（4バイトバッファ + expected長 + 継続バイト蓄積）。

---

### C2: CJK全角文字でカーソルが1しか進まない

**全体像**: `ScreenBuffer::write_char()` が全ての文字に対して `cursor.x += 1` を実行。CJK文字（全角）は2セル幅だが1セルしか進まないため、後続文字が重なる。

**コンセンサス**:
- A3: "文字が2セル占有するのにカーソルが1しか進まず、以降のすべての文字位置がずれる"
- Ge: "This is the most common terminal bug"
- C: "Terminal coordinates are in cells, not Unicode scalar values"
- G: "hardcoded cell_width"

**影響度**: `ls --color`、`vim`、`htop` 等の全フルスクリーンアプリでCJK文字が崩壊。

**修正方針**: `unicode-width` クレート（既にCargo.tomlに依存あり）を使い `UnicodeWidthChar::width()` でセル幅を計算。

---

### C3: Config/ThemeがUI層に到達しない

**全体像**: `config/mod.rs` に `Theme`（3テーマ）+ `FontConfig`（family/size/line_height）が定義されているが、`view.rs` は `const FONT_SIZE: f32 = 14.0` 等のハードコード定数を使用。`app.rs` も独自のカラー定数を再定義。`AppConfig::active_theme()` は一度も呼ばれていない。

**コンセンサス**:
- A4: "設定システムが装飾品と化している"
- C: "Wire AppConfig.font and Theme into the view; remove hardcoded constants"
- G: "Inject config values into TerminalPane during initialization"
- Ge: "Use GPUI's cx.text_style() and line_height metrics"

**影響度**: テーマ切替、フォント変更、Ctrl+ホイールズームが全て不可能。

**修正方針**: `TerminalPane::new()` に `&AppConfig` を追加。Theme の fg/bg を `DEFAULT_FG`/`DEFAULT_BG` 定数から置換。

---

### C4: リサイズがペーンサイズでなくウィンドウ全体を使用

**全体像**: `render()` 内で `window.viewport_size()` からウィンドウ全体のサイズを取得し、40pxを引いてターミナルサイズを計算。スプリットペイン時には各ペインが半分以下のサイズなのに、全ペインが同じ（ウィンドウ全体の）サイズでPTYにリサイズシグナルを送信。

**コンセンサス**:
- A4: "vi、less、htop等のフルスクリーンアプリで表示が崩れる"
- Ge: "Never call handle_resize inside render. PTY resizing involves a syscall"
- C: "if tab bar height changes, terminal rows will be wrong"

**影響度**: マルチペイン環境で全フルスクリーンアプリが正常動作しない。

**修正方針**: GPUI のレイアウトシステム経由で各ペインの実際のBoundsを取得。`Element` trait実装 or `on_resize` コールバック活用。

---

## 詳細分析: Tier 1 HIGH

### H1: unboundedチャネルでメモリ爆発

**問題**: `flume::unbounded()` で `TerminalEvent` チャネル作成。大量出力時（`yes`, `cat /dev/urandom`）に reader スレッドが高速で `OutputReceived` を送信し続け、16ms周期の drain が追いつかない。

**修正**: `flume::bounded(1)` に変更。`try_send` が `Full` なら既に通知済みなので無視でよい（ターミナルは最新状態だけ表示すればよい）。

### H2: Drop時のjoin()がUIスレッドをブロック

**問題**: `TerminalSurface::drop()` で `handle.join()` がタイムアウトなし。ConPTYのEOF伝播がWindows実装依存で遅延する場合、メインスレッドがフリーズ。

**修正**: `AtomicBool` 停止フラグを reader スレッドに渡すか、join をバックグラウンドスレッドに委譲。

### H3 + H6: プロセス終了通知なし + ポーリング継続

**問題**: PTY reader 終了 → `ProcessExited(0)` → view.rs は `dirty = true` にするだけでループ継続。ユーザーは死んだシェルを見たまま放置。

**修正**: `TerminalPane` に `exited: bool` フィールド追加。`ProcessExited` 受信でフラグセット + ループ脱出。render() で "[Process exited]" 表示。

### H4: write_charのO(n²)パディング

**問題**: `while line.text.chars().count() <= x` は毎回文字列全体をスキャン。カーソルが行末に近い場合、繰り返しスキャンでO(n²)。

**修正**: セル長を別途トラッキング（`line.len_cells: usize`）するか、`Vec<char>` に構造変更。

### H5: unsafe as_bytes_mutのString破壊リスク

**問題**: ASCII fast path で `as_bytes_mut()[x]` を直接変更。`is_ascii()` チェックは現時点で正しいが、将来の変更で不整合が生じれば String の UTF-8 不変条件を破壊 → 後続操作でパニック/UB。

**修正**: unsafe を除去し `replace_range` を使用。または `Vec<Cell>` 構造体へ移行。

---

## 反証と議論

### 議論1: "デッドロックは存在するか？"

- **A1の結論**: "真のレース条件（データ競合）は現在の設計には存在しない"
- **Geminiの指摘**: "ClosePseudoConsoleがすぐにEOFを伝播させるかはWindows実装依存"
- **最終判定**: **デッドロックなし、しかしブロッキングjoinによるUIフリーズリスクあり**

### 議論2: "Entity<TerminalPane>のリークは発生するか？"

- **A2の調査**: "GPUI の WeakEntity が正しく使われており、エンティティ解放後はタスクが終了する"
- **A2の補足**: "ただし detach 済みタスクが Entity の生存に依存して長生きする"
- **最終判定**: **完全なリークではないが、プロセス終了後も不要なポーリングが継続する**

### 議論3: "unsafe as_bytes_mut は本当に危険か？"

- **Gemini**: "CRITICAL - extremely unsafe, remove immediately"
- **A5**: "MEDIUM - 理論上の問題だが現時点のガード条件で実害なし"
- **A3**: "LOW - ASCII-only パスでは安全"
- **最終判定**: **HIGH — 現時点では安全だが、CJK対応（C2修正）時に条件が変わり危険化する。C2と同時に修正すべき**

### 議論4: "VTパーサのUTF-8問題はghostty_vt有効時に影響するか？"

- **A3**: "ghostty_vt有効時はghostty側がUTF-8を処理するため影響なし"
- **最終判定**: **ghostty_vt OFF時のみ影響。ただしフォールバックパスとして修正必要**

---

## 推奨修正優先順位

### Phase 6a: 正確性修正（CRITICAL）
1. **C2**: write_char に unicode-width 導入（cursor.x += width）
2. **C1**: VtParser に UTF-8 デコードステートマシン追加
3. **C4**: リサイズをペーン実サイズから計算
4. **H5**: unsafe as_bytes_mut 除去（C2と同時に）

### Phase 6b: 設定統合（CRITICAL）
5. **C3**: TerminalPane に AppConfig/Theme を注入
6. **M1**: cell_width をフォントメトリクスから計算

### Phase 6c: リソース管理（HIGH）
7. **H1**: flume::bounded(1) に変更
8. **H2**: Drop join にタイムアウト or 停止フラグ
9. **H6/H3**: ポーリングループ停止 + プロセス終了UI通知
10. **H7**: ConPTYエラーパスのHANDLE RAII化

### Phase 6d: VTパーサ改善（HIGH）
11. **H8**: 256色モード(6x6x6キューブ + グレースケール)実装
12. **M6**: OSC UTF-8処理
13. **H4**: write_char O(n²)パディング解消

### Phase 6e: アーキテクチャ改善（MEDIUM）
14. **M2**: EventEmitter<TerminalPaneEvent> 型定義
15. **M3**: tab_bar.rs / status_bar.rs 分離
16. **M8**: キーバインド設定化
17. **M4**: ghostty_vt expect() → Result 変換

---

## 統計サマリー

| 重要度 | 件数 | 対応フェーズ |
|--------|------|------------|
| CRITICAL | 4 | 6a, 6b |
| HIGH | 8 | 6c, 6d |
| MEDIUM | 10 | 6e+ |
| LOW | 5 | バックログ |
| **合計** | **27** | |

| 仮説 | 検証結果 |
|------|---------|
| 並行性・レース条件 (A1) | **部分的に支持** — デッドロックなし、チャネル/joinの問題あり |
| メモリ・リソースリーク (A2) | **部分的に支持** — scrollback trimなし、join blockあり |
| VTパーサ・レンダリング (A3) | **強く支持** — UTF-8/CJK/256色の3件CRITICAL |
| アーキテクチャ・拡張性 (A4) | **強く支持** — 設定断絶が全機能追加をブロック |
| エラー処理・堅牢性 (A5) | **部分的に支持** — expect/プロセス終了通知の問題 |
