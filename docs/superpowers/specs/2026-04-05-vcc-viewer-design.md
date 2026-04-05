# VCC Viewer Design

**Date:** 2026-04-05  
**Project:** ZWG Terminal  
**Scope:** Phase 1 only — Mojo-based VCC viewer integrated into ZWG as a workspace tab

## Goal

ZWG Terminal に、`lllyasviel/VCC-experiments` の `VCC.py` 相当機能を **Mojo 化したコア**で取り込み、JSONL 会話ログを `min / full / view` の 3 モードで閲覧・検索できる `VCC Viewer` を追加する。

このフェーズでは、閲覧・検索・成果物生成に集中し、`train.py` / `evaluate.py` 相当の実験ランナーは実装しない。ただし、次段で再利用できる CLI 境界と成果物 manifest はこの段階で整える。

## User-Approved Decisions

- UI の入口は **ワークスペースタブ** とする
- 入力対象は **手動で開いた `*.jsonl`** と **ZWG が保持するログ一覧** の両対応とする
- 表示モードは **`Min / Full / View` の 3 切替** を最初から入れる
- `View` は **検索語を確定した時だけ** 生成する
- 出力先は **元 JSONL と同じ場所** と **ZWG 管理キャッシュ** の両対応、初期値は **元 JSONL と同じ場所**
- Python 依存の持ち込みは行わず、**VCC コンパイル・検索ロジックは Mojo に変換**する

## Non-Goals

- AppWorld 依存の学習基盤導入
- `train` / `evaluate` ランナーの GUI 化
- VCC 全機能の一括移植
- 既存ターミナルタブやスニペットパレットの責務変更

## Product Behavior

### 1. VCC Viewer Tab

ZWG のタブ列に `VCC Viewer` を追加する。通常ターミナルタブとは別種だが、ユーザーにとっては同じワークスペース操作系で使えることを優先する。

左カラムはソース一覧とし、次の 2 種類を同じモデルで表示する。

- 明示的に `Open JSONL` で開いたファイル
- ZWG が保持・検出できる会話ログ

上部ツールバーには以下を置く。

- `Open JSONL`
- `Refresh Logs`
- `Output Location`
- `Compile`
- `Search`

右ペインは 3 モード切替表示とする。

- `Min` — `.min.txt` を表示
- `Full` — `.txt` を表示
- `View` — 検索語確定後に `.view.txt` を表示

`Min` と `View` に含まれる行参照はクリック可能にし、`Full` の該当範囲へジャンプする。

### 2. State Feedback

生成状態はタブ上で明示する。最低限次の状態を持つ。

- `stale` — ソース更新後に成果物が古い
- `compiling` — Mojo コア実行中
- `ready` — 成果物が最新
- `error` — 失敗。エラー内容を表示

`View` は検索語が未確定なら未生成扱いとし、前回検索結果の自動再利用はするが、入力中のたびに再コンパイルはしない。

## Architecture

採用構成は **Rust UI + Rust Bridge + Mojo Core** の三層とする。

### Rust / GPUI Layer

ZWG の UI 層。`VCC Viewer` タブを描画し、ユーザー操作を受けて `VccBridge` に要求を渡す。UI は Mojo を直接呼ばない。

### Rust Bridge Layer

ZWG 内部サービス `VccBridge` を新設する。責務は次の 3 つ。

- ソース解決
- コンパイル要求の実行制御
- 成果物 manifest の読込

`VccBridge` は UI に対して同期・非同期の実行状態を返し、成果物ファイルとジャンプ情報を UI が扱いやすいモデルへ変換する。

### Mojo Core Layer

`VCC.py` 相当の中核処理を Mojo に移植する。パイプラインは次を維持する。

1. `JSONL parse`
2. `merge_chunks`
3. `split_chains`
4. `IR build`
5. `lower(full / min / view)`
6. `artifact write`

互換性の重点は以下。

- `.min.txt` の概要生成ルール
- grep 時の `.view.txt` 出力
- line reference と block range

完全な byte 一致は初期条件にしないが、意味論は壊さない。

## Data Model

### VccSourceRef

ソースは UI から直接パスで扱わず、次の 2 種の列挙体で統一する。

- `ManualFile { path }`
- `ZwgLog { id, path, metadata }`

### VccCompileRequest

コンパイル要求は単一オブジェクトに集約する。

- 対象ソース
- 出力先ポリシー
- 生成対象モード
- 検索語
- truncate 設定

### VccArtifactManifest

Mojo 実行結果は manifest で返す。最低限含める。

- `.txt` のパス
- `.min.txt` のパス
- `.view.txt` のパス
- 生成時刻
- 対象ソース情報
- 行参照ジャンプ用インデックス
- エラー情報

この manifest は第 2 段の実験ランナーでも再利用する。

## File Layout

### Rust Side

新設:

- `crates/zwg-app/src/vcc/mod.rs`
- `crates/zwg-app/src/vcc/bridge.rs`
- `crates/zwg-app/src/vcc/source.rs`
- `crates/zwg-app/src/vcc/manifest.rs`
- `crates/zwg-app/src/vcc/viewer_state.rs`
- `crates/zwg-app/src/vcc/view.rs`

変更:

- `crates/zwg-app/src/main.rs`
- `crates/zwg-app/src/app.rs`
- `crates/zwg-app/src/config/mod.rs`

### Mojo Side

新設:

- `mojo-vcc/main.mojo`
- `mojo-vcc/jsonl_reader.mojo`
- `mojo-vcc/ir.mojo`
- `mojo-vcc/compiler.mojo`
- `mojo-vcc/search.mojo`
- `mojo-vcc/artifacts.mojo`
- `mojo-vcc/README.md`

Mojo 側は `mojo run` で成立させる。Rust との境界は JSON 入出力で固定し、将来ネイティブバイナリ化しても上位層の契約を変えない。

## Configuration Changes

設定変更に該当するため、設定バージョンは必ず上げる。

追加候補:

- `vcc_output_location`
- `vcc_recent_sources`

最初の実装ではこれ以上の設定は増やさない。検索履歴や詳細 VCC パラメータは後回しにする。

## Output Location Policy

2 つのポリシーを持つ。

### Same Directory

元の `jsonl` の隣に `.txt` / `.min.txt` / `.view.txt` を置く。初期値はこちら。

### ZWG Managed Cache

ZWG が管理するキャッシュディレクトリに成果物を置く。元ディレクトリを汚したくない場合に使う。

同一ソースを複数場所へ二重生成しないよう、manifest には現在有効な出力先と生成先を必ず残す。

## Testing Strategy

### Rust Unit Tests

最低限次をカバーする。

- `source resolve`
- `manifest parse`
- `output location policy`
- `stale` 判定
- line reference から `Full` 表示位置へのジャンプ解決

### Mojo Golden Tests

小さな `jsonl` fixture を用い、以下を検証する。

- `.txt`
- `.min.txt`
- `.view.txt`

重点は line reference と grep block range の正しさ。

### UI Tests

可能な範囲で GPUI の既存流儀に合わせて以下をテストする。

- 手動 JSONL 選択
- ログ一覧選択
- `Min / Full / View` 切替
- 検索確定で `View` 生成
- 行参照クリックで `Full` ジャンプ

## Risks and Mitigations

### Mojo での JSON / 正規表現 / テキスト処理差分

`VCC.py` は Python の文字列処理と `re` に強く依存している。Mojo へ移植する際に差分が出やすい。  
対策として、最初は fixture ベースの golden 比較を作り、意味論が壊れた時点で検出できるようにする。

### app.rs の肥大化

既に巨大なため、VCC UI を直書きすると保守不能になる。  
対策として `vcc/view.rs` へ分離し、`RootView` はタブ切替と委譲だけを持つ。

### ログソースの不統一

手動ファイルと ZWG 管理ログが別ロジックで実装されると、UI 条件分岐が増える。  
対策として `VccSourceRef` へ正規化し、UI はソース種類を意識しない構造にする。

## Definition of Done

以下をすべて満たした時点で第 1 段を完了とする。

- ZWG から `VCC Viewer` タブを開ける
- `Open JSONL` で任意の `*.jsonl` を読み込める
- ZWG 管理ログ一覧からも読み込める
- `Min / Full / View` を切り替えられる
- 検索語確定で `View` が生成される
- 出力先を `元ディレクトリ / 管理キャッシュ` で切り替えられる
- 行参照クリックで `Full` の該当位置へ飛べる
- 設定バージョン更新が入る
- 関連テストが通る
- `cargo zigbuild --release -p zwg-app` が通る

## Phase Boundary

第 2 段の実験ランナーはこの spec には含めない。  
ただし次の前提はこの段階で満たす。

- Mojo CLI 境界が再利用可能
- 成果物 manifest が将来のランナーから参照可能
- Rust 側に VCC 機能の責務境界ができている
