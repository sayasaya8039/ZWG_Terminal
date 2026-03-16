# Findings: 定型文入力（Snippet）IME 日本語入力不具合

対象: `D:\NEXTCLOUD\Windows_app\ZWG_Terminal\crates\zwg-app\src\app.rs`
作成日: 2026-03-16

## 目的
定型文機能で日本語入力ができない件について、5体のエージェント調査結果を突き合わせ、合意点を1点に整理する。

## エージェント別仮説（要約）

- A(単体解析): `should_defer_snippet_keystroke_to_ime` の判定が、IME中でも ASCII 文字列を直接文字として扱い、IME処理を回避できていない。
- B(Windows Hook): フック側のイベント検知は `WM_KEYDOWN+VK_PROCESSKEY` 依存で脆い可能性あり。
- C(対象特定): フォーカス/IMEターゲットより、実害は `should_defer` 判定の分岐ロジック依存。
- D(文字種別): `direct_text_from_snippet_keystroke` を通った文字をすべて IME対象外として扱う変更が過剰。
- E(フラグ管理): `swap(false)` の1ショット設計が連続イベント取りこぼしを起こし得る。

※補足: 一部サブエージェントは別リポジトリ文脈を参照しており、当該結果は本件には直接適用しない。

## 合意（ファインド）

今回の修正で最も整合的に説明できる原因は以下。

1. `app.rs` の `should_defer_snippet_keystroke_to_ime` が
   - IME中フラグが立っていても
   - `direct_text_from_snippet_keystroke` を通過した `Some` なら
   - IME対象判定を完全にスキップしていた。
2. その結果、IME入力中に送られるローマ字キー（ASCII 1文字）が「IME経路を経ず」直接文字挿入へ進み、IME合成が壊れる。
3. 一方で、IME確定で入ってくる非ASCII文字（例: `あ`）は引き続き直接挿入で扱える必要がある。

## 実施した変更（最小差分）

- ファイル: `crates/zwg-app/src/app.rs`
- 関数: `should_defer_snippet_keystroke_to_ime`
  - `direct_text...` が `Some` の場合、文字種で振り分け。
  - **ASCII 文字列は IME中は defer（処理保持）**
  - **非ASCII 文字列は IME確定文字として defer を打ち切って直挿入を許可**
- テスト追加
  - `should_defer_snippet_keystroke_to_ime_defers_ascii_key_input`
    - `key="a", key_char="a"` で defer が `true` になることを確認。

## 変更前後の意図比較

- 変更前（今回問題箇所）
  - `direct_text...` が `Some` なら常に defer を中断していた。
- 変更後（本対応）
  - IME中でも `a` 等のASCIIはIMEとして deferred を維持。
  - `あ` 等 non-ASCII は IME確定として直挿入を許可。

## 想定効果

- IME合成中のローマ字入力で「文字が落ちる/即時確定されない」問題の解消。
- 非ASCIIの直接確定文字（IME候補確定）を阻害せず維持。

## 追加観測メモ

- 同じ修正でもし再発する場合は、次に Hook 側 (`WM_KEYDOWN`/`WM_KEYUP` + `VK_PROCESSKEY`) への拡張を検討。
