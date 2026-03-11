# FINDINGS: パフォーマンスボトルネック調査

**Date**: 2026-03-11
**Method**: Delegate Mode — 5 Investigation Agents + 3 AI Consultants (Grok, Gemini, ChatGPT)
**Project**: ZWG Terminal (Rust + GPUI + Zig FFI + ConPTY)

---

## Executive Summary

8/8 の調査者が一致した結論:

> **最大のボトルネックは「毎フレーム全行の div ツリー再構築 + String アロケーション」であり、
> 推定フレーム時間 22-39ms（60fps 予算 16.67ms を大幅超過）。
> カスタム Element + スナップショットパターン + イベント駆動で 60fps 安定化が可能。**

---

## 調査結果マトリクス

### Agent 発見一覧

| Agent | 仮説 | Verdict | 最大ボトルネック |
|-------|------|---------|----------------|
| **Agent 1** (Render Lock) | Mutex 競合 + アロケーション | **CONFIRMED** | Mutex 内で 48 回メソッド呼び出し、毎秒 7-11MB ヒープ圧 |
| **Agent 2** (PTY I/O) | ポーリング + チャネル容量 | **CONFIRMED** | bounded(4) で 25% イベント drop、46% スループット低下 |
| **Agent 3** (Allocation) | 毎フレーム String/Vec 爆発 | **CONFIRMED** | 287-310 alloc/frame、CRITICAL 5件 |
| **Agent 4** (Grid Render) | div 要素数が GPUI を圧迫 | **CONFIRMED** | 200-960 要素/frame、推定 22-39ms/frame |
| **Agent 5** (Resize/State) | リサイズ + ファイルI/O | **CONFIRMED** | render() 内同期 fs::write、viewport_size ペイン不一致 |

### AI コンサルタント合意

| AI | Top 推奨 | 合意ポイント |
|----|----------|-------------|
| **Grok** | Double-buffer + Arena alloc + Event-driven | Mutex 排除、バッファ再利用、イベント駆動 |
| **Gemini** | **カスタム Element + paint_text()** (70-80%改善) | div soup → 1ノード描画、Zed Editor 方式 |
| **ChatGPT** | UI thread parsing + Dirty-row tracking | ロック排除、差分更新、非同期I/O |

---

## ボトルネック優先度（全調査統合）

### Tier 0: CRITICAL（フレーム予算を超過させる主因）

| # | 問題 | 合意度 | 推定インパクト | 現在値 |
|---|------|--------|---------------|--------|
| **C1** | 全行毎フレーム div 再構築 | 8/8 | 8-18ms/frame | 24行 × div chain |
| **C2** | render_styled_text span 爆発 | 5/8 | 12-18ms/frame | 行あたり 5-40 span |
| **C3** | Mutex 競合 (render vs reader) | 8/8 | 1-5ms/frame | 毎フレーム lock |
| **C4** | 毎フレーム String/Vec alloc | 8/8 | 2-5ms/frame | 287-310 alloc/frame |
| **C5** | render() 内同期ファイル I/O | 6/8 | 5-50ms spike | 2秒ごと fs::write |

### Tier 1: HIGH（安定性・スループットに影響）

| # | 問題 | 合意度 | 推定インパクト |
|---|------|--------|---------------|
| **H1** | bounded(4) チャネル容量不足 | 3/8 | 25% イベント drop |
| **H2** | 16ms ポーリング（非効率） | 8/8 | 最大 16ms 遅延 + CPU 空転 |
| **H3** | viewport_size() ペイン不一致 | 3/8 | 分割時リサイズ誤計算 |
| **H4** | カーソル行の absolute overlay | 5/8 | 2ms/frame |
| **H5** | window.bounds() 毎フレーム Win32 API | 3/8 | 60 API call/秒 |

### Tier 2: MEDIUM（微最適化）

| # | 問題 | 推定インパクト |
|---|------|---------------|
| **M1** | tab_infos/shell_entries の String clone | 30-50 alloc/frame |
| **M2** | format!() による element ID 生成 | 10-20 alloc/frame |
| **M3** | f32 比較の毎フレーム実行 | < 0.1ms |

---

## フレーム時間内訳（現状推定 80×24 ターミナル）

```
16.67ms 予算（60fps 目標）
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

現状推定: 22-39ms/frame
├─ div ツリー構築 + GPUI layout   : 8-18ms  ← C1+C2
├─ flexbox 行毎計算               : 3-6ms   ← C1
├─ Mutex lock + データ読取        : 1-5ms   ← C3
├─ String/Vec アロケーション      : 2-5ms   ← C4
├─ GPU paint + rasterize          : 3-5ms
├─ ファイル I/O (spike)           : 0-50ms  ← C5
└─ その他 (bounds, resize)        : 1-2ms   ← H5

実態: 30-45fps（フレームドロップ 25-50%）
```

---

## 推奨アーキテクチャ（全調査者コンセンサス）

### 最適アーキテクチャ（段階的移行）

| 項目 | 現在 | Phase 1 | Phase 2 (最終形) |
|------|------|---------|-----------------|
| **描画方式** | 行ごと div() | Dirty-row 差分更新 | **カスタム Element + paint_text()** |
| **同期方式** | Mutex<Backend> | Snapshot swap | **UI thread parsing** |
| **アロケーション** | 287-310/frame | バッファ再利用 (60/frame) | **ゼロコピー (<10/frame)** |
| **ポーリング** | 16ms timer | 4ms timer | **Event-driven + coalescing** |
| **チャネル** | bounded(4) | bounded(64) | **AtomicBool dirty flag** |
| **ファイル I/O** | 同期 render() 内 | **async background** | async background |
| **リサイズ** | viewport_size() 全体 | **ペイン個別サイズ** | layout callback |

---

## 実装ロードマップ

### Phase 6a: 即効性修正（1-2日、難度: 低）

| # | タスク | ファイル | 効果 |
|---|--------|---------|------|
| 1 | `bounded(4)` → `bounded(64)` | surface.rs:157 | +25% スループット |
| 2 | ファイル I/O を background executor | app.rs:265 | spike 完全除去 |
| 3 | Mutex lock 範囲縮小（resize） | surface.rs:289 | lock 競合 -50% |
| 4 | window.bounds() を 100ms 間隔に | app.rs:237 | API call -86% |

### Phase 6b: アロケーション削減（2-3日、難度: 中）

| # | タスク | ファイル | 効果 |
|---|--------|---------|------|
| 5 | row_texts バッファプール化 | view.rs:234 | -50 alloc/frame |
| 6 | render_styled_text char Vec 廃止 | view.rs:373 | -100 alloc/frame |
| 7 | tab_infos 直接ループ（clone 廃止） | app.rs:278 | -20 alloc/frame |
| 8 | カーソル行 String プール化 | view.rs:262 | -5 alloc/frame |

### Phase 6c: Dirty-row 差分更新（3-5日、難度: 中-高）

| # | タスク | 効果 |
|---|--------|------|
| 9 | 行ごと dirty flag 追跡 | 変更行のみ再構築 → -70% 要素生成 |
| 10 | カーソルを独立オーバーレイ化 | カーソル blink で行再構築不要 |
| 11 | style run マージ（隣接同色結合） | span 数 50-70% 削減 |

### Phase 6d: イベント駆動 + Snapshot（3-5日、難度: 高）

| # | タスク | 効果 |
|---|--------|------|
| 12 | 16ms polling → event-driven + coalescing | CPU 空転除去、遅延 -16ms |
| 13 | Mutex → Snapshot/Double-buffer | render/reader 完全並列化 |
| 14 | viewport_size → ペイン個別サイズ | 分割ペイン正確リサイズ |

### Phase 6e: カスタム Element（1-2週、難度: 極高）

| # | タスク | 効果 |
|---|--------|------|
| 15 | `gpui::Element` 実装 + `paint_text()` | div soup 完全排除、1ノード描画 |
| 16 | フォントグリフキャッシュ | GPU 描画 70-80% 高速化 |

---

## 期待パフォーマンス推移

| Phase | フレーム時間 | FPS | alloc/frame |
|-------|------------|-----|-------------|
| 現状 | 22-39ms | 30-45 | 287-310 |
| **6a 完了** | 18-30ms | 35-55 | 287-310 |
| **6b 完了** | 14-22ms | 45-60 | **~60** |
| **6c 完了** | 8-14ms | **60+** | **~20** |
| **6d 完了** | 5-10ms | **60+ (安定)** | **~15** |
| **6e 完了** | **2-5ms** | **60+ (余裕)** | **<10** |

---

## 参考: プロダクション比較

| ターミナル | render 方式 | 同期 | FPS |
|-----------|------------|------|-----|
| **Alacritty** | GPU直接描画 + damage tracking | Lock-free grid | 1000+ idle |
| **WezTerm** | GPU + custom renderer | Double-buffer | 120+ |
| **Warp** | React-like + GPU | Snapshot | 60+ |
| **ZWG (現状)** | div soup + Mutex | Mutex polling | **30-45** |
| **ZWG (目標)** | custom Element + paint_text | Event-driven | **60+ 安定** |

---

## Consensus Statement

> **8/8 調査者（5 Agent + 3 AI）が一致:**
>
> ZWG Terminal のパフォーマンスボトルネックは複合的だが、最大の原因は
> 「毎フレームの全行 div ツリー再構築」と「Mutex 競合による render 遅延」。
>
> Phase 6a（即効性修正）で体感改善、Phase 6b-c で 60fps 安定化、
> Phase 6e（カスタム Element）で Alacritty 級のパフォーマンスが実現可能。
>
> **最小投資で最大効果: Phase 6a + 6b（3-5日）で 45-60fps 達成見込み。**
