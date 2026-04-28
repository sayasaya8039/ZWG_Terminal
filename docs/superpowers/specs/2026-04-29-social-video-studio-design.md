# Social Video Studio Design

**Date:** 2026-04-29  
**Project:** Social Video Studio  
**Spec Location:** ZWG Terminal docs  
**Implementation Root:** `D:/NEXTCLOUD/Windows_app/SocialVideoStudio`  
**Scope:** Full local operations platform for daily short-form social video packs, implemented as a separate Remotion/React/Node project with future optional ZWG integration

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
- 実装場所は **ZWG Terminal リポジトリ外の独立アプリ** とし、後で ZWG Terminal 連携する。
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
- ZWG Terminal リポジトリ配下に Remotion/React/Node アプリ本体を作ること。
- 音声ナレーション生成。

## Phased Delivery

### Phase 1: Daily Generation Core

ZWG Terminal とは別フォルダの `D:/NEXTCLOUD/Windows_app/SocialVideoStudio` に独立プロジェクトとして作り、毎朝生成の中核を実装する。ZWG Terminal リポジトリには、この設計書と将来連携用の仕様だけを置く。

- 最低限の RSS/API collector
- Manual override collector
- Remotion による 9:16 共通動画生成
- SNS 別コメントとハッシュタグ生成
- SQLite 保存
- `exports/YYYY-MM-DD/` への投稿パック出力
- React/Vite ダッシュボード
- Windows タスクスケジューラから呼べる `generate-today` CLI
- 手動実行ボタン

Phase 1 だけではブラウザ取得や X.com 読み取りを含む本運用要件は満たさない。Phase 1 は「RSS/API と手動差し込みで 10 本生成できる内部 alpha」とし、ユーザー承認済み MVP は Phase 1 と Phase 2 の基礎範囲を合わせて満たす。

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

`D:/NEXTCLOUD/Windows_app/SocialVideoStudio` を独立した実装ルートにし、既存 ZWG Terminal 本体とは疎結合にする。ZWG Terminal との連携は Phase 3 まで実装せず、将来の起動、監視、通知リング連携も明示的な IPC または launcher 境界を通す。

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

`generate-today` は API サーバー非依存の Node CLI として動作する。Windows タスクスケジューラは UI/API サーバーを起動せず、CLI が SQLite と `exports/YYYY-MM-DD/` に直接書き込む。ダッシュボードは後から SQLite と manifest を読み、生成済み成果物を表示する。

タスクスケジューラ登録時は working directory を `D:/NEXTCLOUD/Windows_app/SocialVideoStudio` に固定し、必要なら `--env-file <path>` で明示的に env ファイルを指定できるようにする。

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

collector 共通ポリシー:

- RSS/API と公式公開ページを優先する。
- robots.txt、サイト利用規約、公開 API の rate limit に従う。
- ログイン回避、CAPTCHA 回避、ペイウォール回避、アクセス制限回避は行わない。
- 取得頻度は source ごとに設定し、既定では毎朝 1 回の生成ジョブ内に制限する。
- 保存対象は title、URL、公開日時、短い抜粋、構造化メタデータ、画像候補メタデータに限定する。
- 記事本文の丸ごと保存や再配布をしない。
- 生 HTML や取得本文のキャッシュはデバッグ用の短期保持に限定し、既定保持期間は 30 日以内にする。
- 取得不可、利用規約不明、robots.txt で拒否、rate limit 到達の場合は該当 source を skipped とし、他 source と manual override にフォールバックする。

### X.com Readonly Policy

X.com collector は補助情報源として扱う。外部情報源で候補を作り、ログイン済みブラウザ読み取りはトレンド確認と補強に限定する。

- 読み取り専用にし、投稿、いいね、フォロー、DM、フォーム送信、通知操作はしない。
- アプリは X.com のユーザー名、パスワード、Cookie、セッション情報を保存しない。
- ブラウザプロファイルはユーザー管理とし、アプリは認証情報へ直接アクセスしない。
- ログイン壁、CAPTCHA、警告画面、アカウント制限表示、明示的な取得禁止、異常な rate limit を検出した場合は即座に X.com collector を停止する。
- 停止時は外部情報源と manual override にフォールバックし、UI に「X.com 要確認」と表示する。
- X.com 由来データは trend label、表示テキスト、参照 URL、取得時刻、取得方式だけを保存し、画面全文や個人情報を広く保存しない。

### AI Provider Layer

OpenAI API を優先し、ローカル LLM へ差し替え可能な interface にする。

- 要約
- 字幕構成
- 投稿コメント
- ハッシュタグ
- NG ワードと高リスク表現のチェック

## Data Flow

1. Windows タスクスケジューラが毎朝 6:00 に `generate-today` を起動する。
2. `generate-today` が env、設定、SQLite 接続、出力先を初期化する。
3. collectors が候補を集める。Phase 1 では RSS/API と manual override、MVP 完了時点では RSS/API、公式ページ、ブラウザ取得、X.com 読み取りを含める。
4. 候補を共通形式へ正規化する。
5. 鮮度、話題性、カテゴリ最低枠、重複、出典信頼度、画像利用可否をスコアリングする。
6. 各カテゴリ最低 1 本を確保し、残り枠を話題性スコアで埋めて 10 本を選ぶ。
7. AI provider が動画用テキスト、字幕構成、SNS 別コメント、ハッシュタグを生成する。
8. Remotion が 10 本の共通 9:16 動画を生成する。
9. 信頼度が高いものは投稿パック入り、低いものは要確認にする。
10. SQLite に履歴を保存し、`exports/YYYY-MM-DD/` にファイルを出力する。
11. ダッシュボードで当日分を確認し、コピー、再生成、投稿済み管理を行う。

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

画像候補ごとに最低限次の情報を保存する。

- `sourceUrl`
- `sourcePublisher`
- `licenseStatus`: `allowed` / `unknown` / `restricted`
- `usageDecision`: `embed` / `metadata-only` / `reject`
- `requiresReview`
- `downloadedAt`
- `localPath`

`licenseStatus` が `unknown` または `restricted` の画像は動画へ埋め込まない。`requiresReview` が true の場合は、動画を投稿パックに入れても UI 上では要確認として表示する。

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
- OpenAI API キーなどは `D:/NEXTCLOUD/Windows_app/SocialVideoStudio/.env` または `--env-file` で指定したファイルから読み、Git 管理しない。
- `.env.example` を用意し、必須キー名と任意設定だけを記載する。
- 起動時に必須 secret の不足を検出する。
- OpenAI provider を使う場合は `OPENAI_API_KEY` を必須にし、ローカル LLM provider を使う場合は secret 不要で起動できるようにする。
- Windows タスクスケジューラ登録時は実行ユーザー、working directory、env file path、ログ出力先を明示する。
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
- ZWG Terminal との通知リング連携を Phase 3 のどこで扱うか。
