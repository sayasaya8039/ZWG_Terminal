---
name: x-research
description: Grok (xAI) のx_searchを使ってX(Twitter)のトレンド・投稿を検索し、投稿ネタや情報収集を行う
---

# X Research via Grok (xAI x_search)

## Overview

Grok の x_search 機能を使い、X(Twitter) のリアルタイム投稿を検索・分析する。
Claude Code から xAI API を呼び出し、X投稿の検索専用マイクロサービスとして機能する。

## 使い方

ユーザーが `$topic` を指定する。指定がなければ質問する。

## 実行手順

1. ユーザーから調査トピック（`$topic`）を受け取る
2. 以下のコマンドを実行する:

```bash
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "$topic"
```

### オプション指定

- ロケール指定: `--locale ja`（日本語優先）/ `--locale global`（英語優先）
- 対象読者: `--audience engineer` / `--audience investor` / `--audience both`
- 調査日数: `--days 7`（デフォルト30日）
- ドライラン: `--dry-run`（リクエスト内容を確認）
- 出力先: `--out-dir path/to/dir`

### 例

```bash
# AIトレンドを投資家+エンジニア向けに調査
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "AI Agent 最新トレンド" --audience both --days 7

# Web3関連を英語圏で調査
cd /mnt/d/NEXTCLOUD/x-research-skills && npx tsx scripts/grok_context_research.ts --topic "Web3 DeFi trends" --locale global --audience investor
```

## 出力

- `data/context-research/YYYYMMDD_HHMMSSZ_context.md` - 整形されたContext Pack
- `data/context-research/YYYYMMDD_HHMMSSZ_*_context.json` - メタデータ+レスポンス
- `data/context-research/YYYYMMDD_HHMMSSZ_*_context.txt` - テキスト抽出

## 出力の活用

取得した結果を以下の形で整理してユーザーに提示する:

1. **タイムラインの空気（論点クラスター）** - 3-5個
2. **今日の結論（狙うべきテーマ）** - 3個
3. **素材一覧** - 各素材に以下を含める:
   - URL（X投稿URL or 一次情報URL）
   - 要約（1-2行）
   - エンゲージ指標（likes, retweets, replies, views）
   - なぜ伸びたか（仮説3つまで）
   - 投稿ネタ案（投資家向け1つ、エンジニア向け1つ）
   - フック案（1行を3つ）
   - 注意点

## 注意事項

- 投資助言に見える表現は禁止
- 裏が取れない情報は「未確認」と明記
- 一次情報/公式発表/本人発言を優先
