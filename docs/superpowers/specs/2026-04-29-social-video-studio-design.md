# Social Video Studio Design

**Date:** 2026-04-29  
**Project:** ZWG Terminal  
**Scope:** Full local operations platform for daily short-form social video packs, implemented first as an independent Remotion/React/Node app with future ZWG integration

## Goal

毎朝 6:00 に、AI/LLM、ガジェット、国内ニュース、X.com トレンド、100均、各コンビニ新商品などから 10 本分のショート動画投稿パックを生成し、X、Instagram、TikTok、YouTube Shorts へ手動投稿しやすくするローカル運用基盤を作る。

投稿そのものは MVP では自動化しない。動画、SNS 別コメント、ハッシュタグ、サムネ、出典、確認状態をまとめ、ユーザーが朝に確認、コピー、手動投稿、投稿済み管理を短時間で行える状態にする。

## User-Approved Decisions

- MVP の対象は **収集から下書きまで** とする。
- 情報収集は **RSS/API + ブラウザ自動取得** を使う。
- 10 本の配分は **カテゴリ最低枠 + 話題性による自動配分** とする。
- AI 生成は **台本、字幕構成、投稿コメント、ハッシュタグ** を対象にする。
- **AI 音声は生成しない**。
- 出口は **Web ダッシュボード + 日付フォルダ出力** の両方にする。
- 実装場所は **独立アプリを先に作り、後で ZWG Terminal 連携** とする。
- 毎朝 6:00 の起動は **Windows タスクスケジューラ** を使う。
- AI provider は **OpenAI API 優先 + ローカル LLM 差し替え可能** とする。
- X.com トレンドは **外部情報源 + ログイン済みブラウザ読み取り** の両方を使う。
- SNS 向け出力は **MVP では共通 9:16 動画 + SNS 別コメント** とし、後で媒体別動画最適化を追加する。
- 動画デザインは **Mac 風ブリーフィングをベース** にし、100均、コンビニ、商品系のみ **商品紹介カード型** に寄せる。
- 画像素材は **カテゴリ別に切り替え**、商品系は公式画像が取れる場合のみ使い、出典 URL を保存する。
- 公開前確認は **信頼度スコアで分岐** し、低信頼だけ要確認にする。
- 保存は **SQLite + ファイル出力** とする。
- アプローチは **フル運用基盤** を最終形にし、実装は段階分けする。

## Non-Goals

- MVP で SNS へ自動投稿、いいね、フォロー、DM、予約投稿を実行すること。
- X.com のログイン情報、Cookie、API キーなどの秘密情報をアプリ DB に保存すること。
- ニュース記事本文や画像を権利確認なしに再配布すること。
- ZWG Terminal 本体へ最初から深く組み込むこと。
- 音声ナレーション生成。

## Phased Delivery

### Phase 1: Daily Generation Core

独立した `apps/social-video-studio` サブプロジェクトとして、毎朝生成の中核を作る。

- Remotion による 9:16 共通動画生成
- SNS 別コメントとハッシュタグ生成
- SQLite 保存
- `exports/YYYY-MM-DD/` への投稿パック出力
- React/Vite ダッシュボード
- Windows タスクスケジューラから呼べる `generate-today` CLI
- 手動実行ボタン

### Phase 2: Collection and Trust

情報収集と確認品質を強化する。

- RSS/API collector
- 公式ページ collector
- ブラウザ取得 collector
- X.com 読み取り collector
- カテゴリ最低枠と話題性スコア
- 重複排除
- 信頼度スコア
- 低信頼候補の要確認化
- 画像利用状態と出典管理

### Phase 3: Full Operations Platform

フル運用基盤として拡張する。

- 複数テンプレート管理
- 投稿履歴分析
- 媒体別動画最適化
- 投稿結果メモ
- 将来の SNS API / 予約投稿連携
- ZWG Terminal からの起動、監視、通知リング連携

## Architecture

`apps/social-video-studio` を追加し、既存 ZWG Terminal 本体とは疎結合にする。

### React/Vite Dashboard

朝の手動投稿作業を短くするためのローカル Web UI。

- 当日 10 本の一覧
- カテゴリ、信頼度、状態、要確認理由
- 動画プレビュー
- SNS 別コメント表示とコピー
- ハッシュタグコピー
- 投稿済みチェック
- コメント再生成、動画再生成、候補差し替え
- 履歴、分析、設定

### Node/TypeScript API and CLI

ダッシュボード用 API と、タスクスケジューラ用 CLI を同じアプリ境界で提供する。

- `generate-today`: 収集、選定、AI 生成、レンダリング、出力を一括実行
- `serve`: ダッシュボードと API を起動
- `schedule:install`: Windows タスクスケジューラ登録
- `schedule:run`: 手動検証用に同じ生成ジョブを実行

### Remotion

音声なしのショート動画を生成する。

- 1080x1920 の 9:16 composition
- Mac 風ブリーフィングテンプレート
- 商品紹介カード型テンプレート
- 大きめ字幕、見出し、要点、出典表示
- テキストのはみ出し検出
- still レンダーによる視覚確認

### SQLite and File Export

SQLite を正本にし、手動投稿用の成果物をファイルとして出す。

- SQLite: 候補、選定結果、生成テキスト、レンダー状態、投稿状態、出典、エラー履歴
- `exports/YYYY-MM-DD/`: `mp4`、サムネ、SNS 別投稿文、CSV、JSON manifest

### Collector Layer

collector はプラグイン式に分け、1 つ失敗しても全体を止めない。

- RSS/API collector
- Official site collector
- Browser collector
- X.com readonly collector
- Manual override collector

### AI Provider Layer

OpenAI API を優先し、ローカル LLM へ差し替え可能な interface にする。

- 要約
- 字幕構成
- 投稿コメント
- ハッシュタグ
- NG ワードと高リスク表現のチェック

## Data Flow

1. Windows タスクスケジューラが毎朝 6:00 に `generate-today` を起動する。
2. collectors が候補を集める。
3. 候補を共通形式へ正規化する。
4. 鮮度、話題性、カテゴリ最低枠、重複、出典信頼度、画像利用可否をスコアリングする。
5. 各カテゴリ最低 1 本を確保し、残り枠を話題性スコアで埋めて 10 本を選ぶ。
6. AI provider が動画用テキスト、字幕構成、SNS 別コメント、ハッシュタグを生成する。
7. Remotion が 10 本の共通 9:16 動画を生成する。
8. 信頼度が高いものは投稿パック入り、低いものは要確認にする。
9. SQLite に履歴を保存し、`exports/YYYY-MM-DD/` にファイルを出力する。
10. ダッシュボードで当日分を確認し、コピー、再生成、投稿済み管理を行う。

## Candidate Selection

カテゴリは初期値として次を持つ。

- AI / LLM
- ガジェット
- 国内ニュース
- X.com トレンド
- 100均
- コンビニ新商品

毎朝の 10 本は、カテゴリ最低枠と話題性スコアを組み合わせて選ぶ。カテゴリ最低枠は設定で変更できるようにする。候補が不足したカテゴリは、要確認の不足状態として UI に表示し、他カテゴリで埋めるか手動差し込みできるようにする。

## Video Design

基本テンプレートは Mac 風ブリーフィングとする。

- 明るく静かな情報整理型
- 大きい見出し
- 3 つ程度の要点
- カテゴリラベル
- 出典と取得時刻
- 音声なしでも理解できる字幕構成

商品カテゴリではカード型テンプレートを使う。

- 商品画像枠
- 価格、発売日、販売元
- 買う前に見るポイント
- 保存したくなる短い CTA

ニュース、AI/LLM、X.com トレンドはテキスト中心にし、権利が不明な記事画像は使わない。商品系は公式画像が取得でき、利用条件が許容できる場合だけ差し込む。

## Dashboard UX

ダッシュボードは Mac 風の静かな運用画面にする。マーケティングページではなく、毎朝の確認、コピー、投稿済み管理を最短化する。

### Today

- 当日 10 本の一覧
- 動画生成状態
- 信頼度
- 要確認理由
- 投稿済みチェック
- ファイルを開く

### Detail

- 動画プレビュー
- 出典 URL
- 収集元
- 生成要約
- SNS 別コメント
- ハッシュタグ
- サムネ
- 再生成操作

### Settings

- 情報源
- カテゴリ最低枠
- NG ワード
- AI provider
- 出力先
- スケジュール
- X.com 取得方式

### Analytics

- 投稿済み履歴
- スキップ理由
- 再生成回数
- カテゴリ別本数
- 将来の反応メモ

## Security and Compliance

- X.com は読み取り専用にする。
- 投稿、いいね、フォロー、DM、フォーム送信は自動化しない。
- OpenAI API キーなどは `.env` に置き、Git 管理しない。
- 起動時に必須 secret の不足を検出する。
- API キー、Cookie、認証情報をログに出さない。
- 各動画に出典 URL と画像利用状態を保存する。
- ニュース系画像は原則使わず、テキスト中心にする。
- 商品系画像は公式画像が取れる場合のみ使う。
- NG ワード、誇張表現、未確認断定、高リスク表現を検出し、該当候補は要確認にする。

## Error Handling

- collector 単位で失敗を記録し、他 collector は継続する。
- カテゴリ不足は要確認状態として表示する。
- AI provider が失敗した場合は再試行し、失敗継続時は該当候補だけ要確認にする。
- Remotion レンダリング失敗は候補単位で記録し、他動画の生成を継続する。
- 出力失敗は manifest とログに残し、UI から再実行できるようにする。
- タスクスケジューラ実行時も、標準出力、標準エラー、アプリログに原因を残す。

## Testing Strategy

### Unit Tests

- カテゴリ選定
- 話題性と信頼度スコア
- 重複排除
- NG ワード判定
- SNS 別コメント整形
- 出力パス生成

### Integration Tests

- SQLite 保存と読み戻し
- 日付フォルダ出力
- collector 失敗時の継続
- AI provider 差し替え
- Remotion レンダーキュー
- manifest 生成

### E2E Tests

- Today 画面で当日 10 本を確認
- SNS 別コメントコピー
- 投稿済みチェック
- コメント再生成
- 動画再生成
- ファイルを開く導線

### Render Verification

- Remotion still で主要フレームを確認
- 短いサンプル動画をレンダー
- 字幕と長い見出しがはみ出さないことを確認
- 商品テンプレートとブリーフィングテンプレートの両方を確認

### Schedule Verification

- Windows タスクスケジューラ登録コマンドを検証する。
- 手動実行で同じ `generate-today` ジョブが動くことを確認する。
- 失敗時ログが残ることを確認する。

## Open Questions for Implementation Planning

- Phase 1 で使う具体的な UI ライブラリ。
- SQLite ORM または query builder の選定。
- ブラウザ取得に使う実行基盤。
- OpenAI のモデル初期値。
- ローカル LLM の初期対応範囲。
- 商品画像の利用可否判定ルールの細部。
- ZWG Terminal との通知リング連携を Phase 3 のどこで扱うか。

