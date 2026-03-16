---
name: x-trend-ideas
description: Grok x_searchでXのトレンドを探索し、投稿ネタ（impressions最大化）を生成する
---

# X Trend Ideas - 投稿ネタ発見

## Overview

Grok の x_search を使って X のタイムラインの「空気」を読み、impressions を最大化する投稿ネタを生成する。
`/x-research` が記事執筆の前工程リサーチであるのに対し、こちらは X 投稿に特化したネタ出し。

## 使い方

ユーザーが `$topic` を指定する。指定がなければ「AI / Web3 / 開発者ツール」をデフォルトとする。
`$count` は素材数。デフォルト5件。

## 実行手順

以下のスクリプトを実行する。トピックと件数はユーザー指定に合わせて調整する。

```bash
cd /mnt/d/NEXTCLOUD/x-research-skills && XAI_API_KEY=$(grep XAI_API_KEY .env | cut -d= -f2) node -e "
const topic = process.argv[1];
const count = process.argv[2] || '5';
const now = new Date();
const yesterday = new Date(now - 86400000);
const today = now.toISOString().slice(0,10);
const yest = yesterday.toISOString().slice(0,10);

const prompt = \`日本語で回答して。

目的: X(Twitter)でimpressionsを最大化するための投稿ネタ出し。
前提:
- アカウント: 個人発信
- 想定読者: 投資家 + エンジニア
- 領域: \${topic}
- 文体: 常体、ストーリー薄め、結論先出し
- 期間: 昨日と今日 = \${yest} と \${today}（直近24時間を目安）

やること（空気を拾うための探索手順）:
1) まず「広く薄く」探索して、タイムラインの空気（論点のクラスター）を抽出する:
   - AI/Web3/開発者ツール文脈に対して、広めのクエリを12個以上自分で作って X 検索する
   - 収集した投稿から「繰り返し出てくる固有名詞/機能名/言い回し」を抽出し、3-5クラスターにまとめる
   - さらに、上で抽出した「繰り返し出てくる機能名/短いフレーズ」を2-5個選び、追加検索して補強する
   - 可能ならXの検索オペレータを使って「バズ」を拾う（例: min_faves:500, min_retweets:100, since:\${yest}）
2) 次に、クラスターごとに代表ポストを2つずつ選ぶ（長文の直接引用はしない）
3) 合計\${count}件の「素材」を出す（AIとWeb3は偏らせない）
4) 各素材ごとに以下を必ず出す:
- url（Xの投稿URL。無ければ一次情報URL）
- 要約（1-2行、自分の言葉）
- エンゲージ指標（観測できたものだけ。例: likes=?, retweets=?, replies=?, views=?。不明は unknown）
- なぜ伸びたか（仮説を3つまで）
- ここから作れる投稿ネタ案（投資家向け1つ、エンジニア向け1つ）
- フック案（1行を3つ）
- 注意（断定/投資助言に見えない言い回しへ調整点があれば1行）

追加の要求（空気感を出す）:
- 最初に「タイムラインの空気（論点のクラスター）」を3-5個、各クラスターに代表ポストURLを2つずつ付ける
- 「投稿者が使っている言い回し/キーフレーズ」を各クラスターにつき2-3個（そのまま引用せず、短い言い換えで）
- 不確かなゴシップは避け、一次情報/公式発表/本人発言を優先。裏が取れない場合は「未確認」と明記
- 投資助言に見える表現は禁止

出力形式:
- 最初に「タイムラインの空気（論点のクラスター）」を箇条書き
- 次に「今日の結論（狙うべき3テーマ）」を箇条書き
- 次に「素材一覧」を番号付きで\${count}件
- 最後に url だけの一覧をまとめて
\`;

fetch('https://api.x.ai/v1/responses', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': 'Bearer ' + process.env.XAI_API_KEY
  },
  body: JSON.stringify({
    model: 'grok-4-1-fast-reasoning',
    input: prompt,
    tools: [{ type: 'x_search' }]
  })
}).then(r => r.json()).then(data => {
  if (data.output) {
    for (const item of data.output) {
      if (item.content) {
        for (const c of item.content) {
          if (c.text) console.log(c.text);
        }
      }
    }
  } else {
    console.log(JSON.stringify(data, null, 2));
  }
}).catch(e => console.error(e));
" "$topic" "$count"
```

## 出力の活用

Grokからの出力を受け取った後、以下を整理してユーザーに提示する:

1. **タイムラインの空気** - クラスター3-5個 + 代表URL
2. **狙うべき3テーマ**
3. **素材一覧** - 各Candidateを以下の形式で:
   - Title/Angle
   - Claim (1 sentence)
   - Sources (URLs)
   - Specifics (3 bullets)
4. **URL一覧**

## 注意事項

- 投資助言に見える表現は禁止（買い/売り推奨、株価や価格の目標・倍化など）
- 投資家向けネタ案は「論点/評価軸/事業インパクト」の形で書く
- 裏が取れない場合は「未確認」と明記
