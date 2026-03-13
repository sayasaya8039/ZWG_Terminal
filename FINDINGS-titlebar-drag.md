# Findings: タイトルバードラッグが動作しない問題

**日時**: 2026-03-13
**プロジェクト**: ZWG Terminal
**調査方法**: Delegate Mode (5 Agent + 3 AI Consensus)

---

## 調査体制

### 5 Investigation Agents
| Agent | 仮説 | 結果 |
|-------|------|------|
| H1 | WM_NCHITTEST が HTCAPTION を正しく返さない | **REFUTED** — 実装は正しい |
| H2 | appears_transparent がネイティブタイトルバーを適切に除去しない | **CONFIRMED** — WS_CAPTION未除去だが、WM_NCCALCSIZEで回避 |
| H3 | 子要素がヒットテストを消費し親のWindowControlAreaが検出されない | **PARTIALLY CONFIRMED** — タイミング問題あり |
| H4 | TitlebarOptions の設定組合せが不足 | **PARTIALLY CONFIRMED** — traffic_light_position 未設定 |
| H5 | Zed は異なるパターンを使用 | **CONFIRMED** — Zedは window_control_area + start_window_move の二重戦略 |

### 3 AI Consultants
| AI | 主要指摘 |
|----|---------|
| **Grok** | 子要素のヒットテスト干渉が最有力。Zedは非インタラクティブ領域にDrag設定 |
| **Gemini** | ヒットテスト遮蔽（deepest child wins）。Background Layer パターンで解決 |
| **ChatGPT** | 子要素がヒットテストを奪い、親のDragが参照されない。空flex_1 spacerで診断可能 |

---

## コンセンサス：根本原因（合意度 8/8）

### 原因1: `start_window_move()` は Windows では空実装（NO-OP）

**ファイル**: `gpui-0.2.2/src/platform.rs:536`
```rust
fn start_window_move(&self) {} // デフォルト実装 = 何もしない
```

**Windows プラットフォーム**: `start_window_move()` のオーバーライドなし（Linux/Wayland専用API）

**現在のコード** (`app.rs:6004-6008`):
```rust
let title_bar = div()
    .id("title-bar")
    .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
        window.start_window_move();  // ← Windows では何もしない！
    })
```

### 原因2: `window_control_area(WindowControlArea::Drag)` がタイトルバーに未設定

**Zed の実装** (`platform_title_bar.rs:117`):
```rust
let title_bar = h_flex()
    .window_control_area(WindowControlArea::Drag)  // ← WM_NCHITTEST で HTCAPTION を返す
    .on_mouse_down(...)                              // ← Linux フォールバック
    .on_mouse_move(cx.listener(move |this, _ev, window, _| {
        if this.should_move {
            window.start_window_move();              // ← Linux/Wayland 用
        }
    }))
```

**ZWG の現状**:
- `window_control_area(Drag)` なし
- `start_window_move()` のみ（Windows では空）
- → ドラッグ不可能

---

## Windows でのウィンドウドラッグの仕組み

```
マウスクリック → OS が WM_NCHITTEST 送信
  → GPUI handle_hit_test_msg() (events.rs:856)
    → is_movable チェック（true = OK）
    → hit_test_window_control コールバック呼出
      → rendered_frame.window_control_hitboxes を走査
      → mouse_hit_test.ids に hitbox ID が含まれるか確認
      → WindowControlArea::Drag → HTCAPTION 返却
    → OS がウィンドウ移動を開始
```

**キーポイント**: `window_control_area(Drag)` を設定しないと、`window_control_hitboxes` にエントリが追加されず、コールバックは常に `None` を返す → `HTCLIENT` が返却される → OS はドラッグしない。

---

## 修正方針

### 必須修正（全Agent・全AIが合意）

1. **タイトルバー div に `window_control_area(WindowControlArea::Drag)` を追加**
   - `app.rs:6004` の title_bar div
   - `app.rs:6042` のスペーサー div

2. **`start_window_move()` は残す**（Linux互換性のため）

3. **子要素にも Drag を伝播させる**（ヒットテスト遮蔽対策）
   - タブバーの空きスペースにも `window_control_area(Drag)` を設定

### オプション修正

4. `traffic_light_position` の設定（H4指摘、macOS互換性）

---

## 補足：ヒットテスト動作（H3 + Lead独自調査）

`window.rs:775-797` の `hit_test()` は全ヒットボックスを走査し、`BlockMouse` でのみ停止。
通常の子要素（`HitboxBehavior::Normal`）は親のヒットボックスをブロックしない。
したがって、`window_control_area(Drag)` を正しく設定すれば、子要素があっても動作する。

ただし `mouse_hit_test` は直前の `WM_MOUSEMOVE` 時点の位置で計算されるため、
1ピクセル分のタイミング遅延がある（実用上問題なし）。
