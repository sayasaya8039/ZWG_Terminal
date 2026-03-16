---
name: note-write
description: note記事の自動作成ワークフロー（リサーチ→構成→執筆→レビュー）
---

# note記事自動作成ワークフロー

## Overview

5カテゴリ（ツール紹介、エンタメ、ライフスタイル、日本情勢、Xトレンド）から今日の人気トピックを3つずつ抽出し、リサーチ→構成→執筆→レビューの一連を自動化する。

## 使い方

ユーザーが `/note-write` を実行する。オプションで `$genre`（カテゴリ指定）や `$topic`（トピック直接指定）を渡せる。

## 実行フロー

### Phase 1: トレンドリサーチ（並行実行）

以下を**すべて並行**で実行する（1a〜1cを同時起動、直列にしない）:

#### 1a. WebSearch（5カテゴリ同時）

```
WebSearch: "今日 人気 ツール 紹介 2026"
WebSearch: "今日 エンタメ トレンド ニュース 2026"
WebSearch: "今日 ライフスタイル トレンド 2026"
WebSearch: "今日 日本 ニュース 注目 2026"
WebSearch: "今日 X Twitter トレンド バズ 2026"
```

#### 1b. Grok x_search（並行・必須）

```bash
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "ツール紹介 エンタメ ライフスタイル 日本情勢 Xトレンド 今日の人気" --audience both --days 1
```

#### 1c. /x-research スキル（並行・必須）

1b と同時に `/x-research` スキルも実行する。カテゴリごとに個別トピックで呼び出す:

```bash
# 各カテゴリを個別に x-research（Task Bash サブエージェントで並行実行）
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "今日の注目ツール AI 開発者向け" --audience engineer --days 1
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "今日のエンタメ 芸能 アニメ 映画 話題" --audience both --days 1
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "ライフスタイル 暮らし トレンド 話題" --audience both --days 1
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "日本 政治 経済 社会 ニュース 今日" --audience investor --days 1
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "X Twitter バズ トレンド 今日" --audience both --days 1
```

> **注意**: Phase 1 のリサーチでは WebSearch・Grok x_search・/x-research を**必ず同時並行**で実行すること。直列実行は禁止。

### Phase 2: トピック選定

リサーチ結果から各カテゴリ3つずつ、計15トピックを抽出して提示:

```
## 今日のトピック候補（各カテゴリ3つ）

### ツール紹介
1. [トピック名] - 概要（なぜ今日注目か）
2. ...
3. ...

### エンタメ
1. ...（同様）

### ライフスタイル
1. ...

### 日本情勢
1. ...

### Xトレンド
1. ...
```

ユーザーに選択を求める。指定がなければ各カテゴリの1位を採用。

### Phase 3: 記事構成

選択されたトピックに対し、`/mnt/d/NEXTCLOUD/note/templates/` のテンプレートを参照して構成案を生成:

```markdown
# [タイトル案3つ]

## 導入（フック）
- 読者の関心を引く1文
- 背景・なぜ今このトピックか

## 本文
### セクション1: [小見出し]
### セクション2: [小見出し]
### セクション3: [小見出し]

## まとめ
- 要点の整理
- 読者へのアクション喚起

## メタ情報
- 想定文字数: 2000-4000字
- ターゲット読者:
- SEOキーワード:
- ハッシュタグ案:
```

### Phase 4: 執筆

構成案に基づいて記事本文を生成する。

#### 執筆ルール
- 文体: 常体（だ・である調）or 敬体（です・ます調）→読者層に合わせる
- 1文は60字以内を目安
- 段落は3-4文で区切る
- 見出し（h2/h3）を適切に配置
- 箇条書きを活用して読みやすく
- 具体的な数字・事例を入れる
- 結論先出し

#### 出力先
```
/mnt/d/NEXTCLOUD/note/drafts/YYYY-MM-DD/[slug].md
```
ファイル書き込みにはsw.exeを使用する。
保存先は執筆日のフォルダを作成してその配下に置く。

```
例: 2026-02-15/tool-review-chatgpt-5.md
```

### Phase 5: レビュー

執筆した記事を以下の観点でセルフレビュー:

| チェック項目 | 基準 |
|-------------|------|
| タイトル | 30字以内、具体的、好奇心を刺激 |
| 導入 | 3行以内で本題に入っているか |
| 構成 | 論理的な流れがあるか |
| 具体性 | 数字・事例・引用があるか |
| 読了時間 | 5-8分（2000-4000字） |
| SEO | キーワードが自然に入っているか |
| 正確性 | 事実誤認がないか |
| 投資助言 | 投資助言に見える表現がないか |
| 出典 | 情報源が明記されているか |

レビュー結果を付記し、修正が必要なら自動修正して最終版を出力。

## 注意事項

- 投資助言に見える表現は禁止
- 裏が取れない情報は「未確認」と明記
- 一次情報/公式発表を優先
- 出典URLを必ず記載
