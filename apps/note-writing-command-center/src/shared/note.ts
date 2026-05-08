export const ARTICLE_STATUSES = [
  "idea",
  "draft",
  "revising",
  "ready",
  "published",
] as const;

export type ArticleStatus = (typeof ARTICLE_STATUSES)[number];

export type EditorMode = "markdown" | "preview" | "noteHtml";

export type AssistantTask =
  | "outline"
  | "titles"
  | "readability"
  | "thumbnail"
  | "preflight"
  | "format";

export type Article = {
  id: string;
  title: string;
  status: ArticleStatus;
  ideaMemo: string;
  markdown: string;
  thumbnailBrief: string;
  reflection: string;
  tags: string[];
  createdAt: string;
  updatedAt: string;
  publishedAt?: string;
};

export type ChecklistItem = {
  label: string;
  passed: boolean;
  detail: string;
};

export type StatusSummary = Record<ArticleStatus, number>;

export const STATUS_LABELS: Record<ArticleStatus, string> = {
  idea: "ネタメモ",
  draft: "下書き",
  revising: "推敲中",
  ready: "投稿待ち",
  published: "公開済み",
};

export const EDITOR_MODE_LABELS: Record<EditorMode, string> = {
  markdown: "Markdown",
  preview: "プレビュー",
  noteHtml: "note貼り付け",
};

export const ASSISTANT_TASK_LABELS: Record<AssistantTask, string> = {
  outline: "構成提案",
  titles: "タイトル案",
  readability: "読みやすさ",
  thumbnail: "サムネ指示",
  preflight: "投稿前チェック",
  format: "note向け整形",
};

export function createEmptyArticle(): Article {
  const now = new Date().toISOString();
  return {
    id: cryptoSafeId("note"),
    title: "新しいnote記事",
    status: "idea",
    ideaMemo: "読者の悩み、体験、結論をここにメモする。",
    markdown:
      "# 新しいnote記事\n\n## 読者の前提\n\n## 伝えたい結論\n\n## 具体例\n\n## まとめ\n",
    thumbnailBrief: "テーマ、読後感、入れたい文字、避けたい表現を整理する。",
    reflection: "",
    tags: ["note", "執筆"],
    createdAt: now,
    updatedAt: now,
  };
}

export function createSeedArticles(): Article[] {
  const base = "2026-05-08T00:00:00.000Z";
  return [
    {
      id: "article-quiet-workflow",
      title: "毎朝30分でnoteを書き始めるための小さな儀式",
      status: "draft",
      ideaMemo:
        "忙しい会社員が、白紙の怖さを減らして朝に本文へ入るための手順を書く。",
      markdown:
        "# 毎朝30分でnoteを書き始めるための小さな儀式\n\n朝の執筆は、気合いよりも入口の設計で決まります。\n\n## 最初の5分で昨日の素材を見る\n\n前日に残した一文、読者の悩み、結論だけを見返します。ここで新しい調査を始めないことが大切です。\n\n## 10分で見出しを並べる\n\n見出しは完成形ではなく、本文へ降りるための足場です。順番はあとで直せます。\n\n## 15分で一番書ける段落だけ書く\n\n冒頭から書く必要はありません。具体例や失敗談から入ると、自然に本文の温度が上がります。\n\n## まとめ\n\n朝の30分は、完成ではなく前進を作る時間です。明日の自分が戻りやすい一文を最後に置きましょう。\n",
      thumbnailBrief:
        "朝の机、ノート、淡い光。文字は「朝30分のnote習慣」。静かで実務的な雰囲気。",
      reflection: "",
      tags: ["執筆習慣", "note", "朝活"],
      createdAt: base,
      updatedAt: base,
    },
    {
      id: "article-review-checklist",
      title: "投稿前チェックリストでnoteの読み落としを減らす",
      status: "ready",
      ideaMemo:
        "公開直前に迷いがちなタイトル、導入、見出し、貼り付け崩れを1画面で見る話。",
      markdown:
        "# 投稿前チェックリストでnoteの読み落としを減らす\n\n公開前の不安は、才能ではなく確認項目で減らせます。\n\n## タイトルは読者の状況から始める\n\n自分の主張より、読者が今いる場所を先に置くとクリック前の摩擦が下がります。\n\n## 導入で約束を一つに絞る\n\n記事が何を解決するのかを一文で言える状態にします。\n\n## note貼り付け後に見出し間隔を見る\n\nMarkdownからHTMLへ変換した後、改行やリストの詰まりを確認します。\n\n## 最後に次の行動を置く\n\n読後に何を試せばよいかが見えると、保存や共有につながります。\n",
      thumbnailBrief:
        "チェックリストと公開ボタンのイメージ。文字は「投稿前に見る7項目」。",
      reflection: "",
      tags: ["チェックリスト", "note運用"],
      createdAt: base,
      updatedAt: base,
    },
    {
      id: "article-published-retro",
      title: "公開後メモを残すと次の記事が楽になる",
      status: "published",
      ideaMemo:
        "数字だけではなく、反応、書きにくかった箇所、次の仮説を残す運用。",
      markdown:
        "# 公開後メモを残すと次の記事が楽になる\n\n公開して終わりにすると、得られた学びが次の記事に残りません。\n\n## 反応は量より種類で見る\n\nスキ、コメント、SNSでの引用を分けて記録します。\n\n## 書きにくかった箇所を残す\n\n次回の構成づくりで同じ詰まりを避けられます。\n\n## 次の仮説に変える\n\n振り返りは反省ではなく、次のネタを作る材料です。\n",
      thumbnailBrief:
        "公開後のメモ帳と小さな分析グラフ。文字は「公開後メモ」。",
      reflection:
        "導入に対する反応が多かった。次はチェックリスト形式にすると保存されやすそう。",
      tags: ["振り返り", "note分析"],
      createdAt: base,
      updatedAt: base,
      publishedAt: "2026-05-01T09:00:00.000Z",
    },
  ];
}

export function summarizeStatuses(articles: Article[]): StatusSummary {
  return ARTICLE_STATUSES.reduce<StatusSummary>(
    (summary, status) => ({
      ...summary,
      [status]: articles.filter((article) => article.status === status).length,
    }),
    {
      idea: 0,
      draft: 0,
      revising: 0,
      ready: 0,
      published: 0,
    },
  );
}

export function replaceArticle(articles: Article[], next: Article): Article[] {
  const stamped = { ...next, updatedAt: new Date().toISOString() };
  return articles.map((article) => (article.id === next.id ? stamped : article));
}

export function addArticle(articles: Article[], article: Article): Article[] {
  return [article, ...articles];
}

export function extractOutline(markdown: string): string[] {
  return markdown
    .split(/\r?\n/)
    .map((line) => line.match(/^(#{1,3})\s+(.+)$/))
    .filter((match): match is RegExpMatchArray => Boolean(match))
    .map((match) => `${"  ".repeat(match[1].length - 1)}${match[2].trim()}`);
}

export function buildSuggestedOutline(article: Article): string {
  const theme = article.title.trim() || "note記事";
  return [
    `# ${theme}`,
    "",
    "## 読者の悩み",
    "",
    "## この記事で持ち帰れること",
    "",
    "## 背景と具体例",
    "",
    "## 今日から試せる手順",
    "",
    "## まとめ",
    "",
  ].join("\n");
}

export function markdownToNoteHtml(markdown: string): string {
  const html: string[] = [];
  let listItems: string[] = [];

  const flushList = () => {
    if (listItems.length === 0) {
      return;
    }
    html.push(`<ul>${listItems.map((item) => `<li>${inlineMarkdown(item)}</li>`).join("")}</ul>`);
    listItems = [];
  };

  for (const rawLine of markdown.split(/\r?\n/)) {
    const line = rawLine.trimEnd();
    if (!line.trim()) {
      flushList();
      continue;
    }

    const heading = line.match(/^(#{1,3})\s+(.+)$/);
    if (heading) {
      flushList();
      const level = heading[1].length;
      html.push(`<h${level}>${inlineMarkdown(heading[2].trim())}</h${level}>`);
      continue;
    }

    const list = line.match(/^[-*]\s+(.+)$/);
    if (list) {
      listItems = [...listItems, list[1]];
      continue;
    }

    const quote = line.match(/^>\s?(.+)$/);
    if (quote) {
      flushList();
      html.push(`<blockquote>${inlineMarkdown(quote[1])}</blockquote>`);
      continue;
    }

    flushList();
    html.push(`<p>${inlineMarkdown(line)}</p>`);
  }

  flushList();
  return html.join("\n");
}

export function buildPlainPreview(markdown: string): string {
  return markdown
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/\*(.+?)\*/g, "$1")
    .replace(/\[(.+?)\]\((.+?)\)/g, "$1")
    .trim();
}

export function generateTitleCandidates(article: Article): string[] {
  const base = article.title.replace(/^#+\s*/, "").trim() || "note記事";
  const topic = firstMeaningfulTag(article.tags) ?? keyPhrase(article.ideaMemo || article.markdown);
  return [
    `${base}を無理なく続ける方法`,
    `${topic}で迷ったときに見直す小さな手順`,
    `書く前に整える${topic}の考え方`,
    `${base}: 今日から試せる実務メモ`,
    `${topic}をnoteで伝えるためのチェックリスト`,
  ].filter(uniqueByValue);
}

export function buildThumbnailPrompt(article: Article): string {
  const title = article.title.trim() || "note記事";
  const tags = article.tags.length > 0 ? article.tags.join("、") : "執筆、note";
  const brief = article.thumbnailBrief.trim() || "本文テーマが一目で伝わるサムネイル";
  return [
    "noteサムネイル制作指示",
    `主題: ${title}`,
    `キーワード: ${tags}`,
    `画面要素: ${brief}`,
    "トーン: macOS風の静かな作業感、余白多め、文字は短く高コントラスト",
    "避けること: 情報量過多、派手な煽り、note本文と無関係な写真",
  ].join("\n");
}

export function buildPreflightChecklist(article: Article): ChecklistItem[] {
  const markdown = article.markdown.trim();
  const outline = extractOutline(markdown);
  const paragraphs = markdown.split(/\n{2,}/).filter((part) => part.trim().length > 0);
  const hasCta = /試|始め|確認|保存|書いて|やって|次/.test(markdown.slice(-320));
  const longestLine = markdown
    .split(/\r?\n/)
    .reduce((max, line) => Math.max(max, line.trim().length), 0);

  return [
    {
      label: "タイトル",
      passed: article.title.trim().length >= 12 && article.title.trim().length <= 42,
      detail: "12〜42文字を目安に、読者の状況か得られる結果を入れる。",
    },
    {
      label: "見出し構成",
      passed: outline.filter((line) => !line.startsWith("  ")).length >= 3,
      detail: "H2以上の見出しを3つ以上置き、流れを見える状態にする。",
    },
    {
      label: "本文量",
      passed: markdown.length >= 700,
      detail: "noteでは導入、具体例、まとめまで含めて700字以上を初期目安にする。",
    },
    {
      label: "段落密度",
      passed: paragraphs.length >= 5 && longestLine <= 140,
      detail: "長すぎる段落を避け、スマホで読みやすい余白を残す。",
    },
    {
      label: "サムネイル指示",
      passed: article.thumbnailBrief.trim().length >= 20,
      detail: "主題、入れたい文字、避けたい表現が分かる指示を残す。",
    },
    {
      label: "読後アクション",
      passed: hasCta,
      detail: "最後に読者が試すこと、保存する理由、次の行動を置く。",
    },
  ];
}

export function buildReadabilitySuggestions(article: Article): string[] {
  const suggestions: string[] = [];
  const markdown = article.markdown;
  const longLines = markdown.split(/\r?\n/).filter((line) => line.trim().length > 140);
  const outline = extractOutline(markdown);

  if (longLines.length > 0) {
    suggestions.push("140文字を超える行があるため、スマホ表示では2〜3文で改段落する。");
  }
  if (outline.length < 4) {
    suggestions.push("見出しが少ないため、読者の悩み、結論、具体例、手順に分ける。");
  }
  if (!/です|ます/.test(markdown.slice(0, 280))) {
    suggestions.push("導入の語尾を少し整え、読者へ話しかける調子を作る。");
  }
  if (!/例えば|具体的|たとえば/.test(markdown)) {
    suggestions.push("具体例を1つ追加し、主張だけで終わらない本文にする。");
  }

  return suggestions.length > 0
    ? suggestions
    : ["本文の段落、見出し、具体例のバランスは初稿として扱いやすい状態です。"];
}

export function buildAssistantPrompt(task: AssistantTask, article: Article): string {
  return [
    "あなたはnote記事の編集者です。日本語で、短く実用的に返答してください。",
    `依頼: ${ASSISTANT_TASK_LABELS[task]}`,
    `タイトル: ${article.title}`,
    `状態: ${STATUS_LABELS[article.status]}`,
    `ネタメモ: ${article.ideaMemo}`,
    `タグ: ${article.tags.join(", ")}`,
    "本文Markdown:",
    article.markdown.slice(0, 6000),
  ].join("\n\n");
}

export function localAssistantResponse(task: AssistantTask, article: Article): string {
  switch (task) {
    case "outline":
      return buildSuggestedOutline(article);
    case "titles":
      return generateTitleCandidates(article)
        .map((title, index) => `${index + 1}. ${title}`)
        .join("\n");
    case "readability":
      return buildReadabilitySuggestions(article)
        .map((item) => `- ${item}`)
        .join("\n");
    case "thumbnail":
      return buildThumbnailPrompt(article);
    case "preflight":
      return buildPreflightChecklist(article)
        .map((item) => `${item.passed ? "OK" : "要確認"}: ${item.label} - ${item.detail}`)
        .join("\n");
    case "format":
      return [
        "note貼り付け前の整形メモ",
        "- H2/H3の階層を崩さない",
        "- 箇条書きの前後に空行を残す",
        "- 太字は強調箇所だけに絞る",
        "- 変換HTMLを貼り付け後、見出し間隔とリスト表示を確認する",
      ].join("\n");
  }
}

export function sanitizeArticle(input: unknown): Article | null {
  if (!input || typeof input !== "object") {
    return null;
  }
  const value = input as Partial<Article>;
  if (
    typeof value.id !== "string" ||
    typeof value.title !== "string" ||
    !ARTICLE_STATUSES.includes(value.status as ArticleStatus) ||
    typeof value.markdown !== "string"
  ) {
    return null;
  }

  const now = new Date().toISOString();
  return {
    id: value.id,
    title: value.title.slice(0, 160),
    status: value.status as ArticleStatus,
    ideaMemo: typeof value.ideaMemo === "string" ? value.ideaMemo : "",
    markdown: value.markdown,
    thumbnailBrief:
      typeof value.thumbnailBrief === "string" ? value.thumbnailBrief : "",
    reflection: typeof value.reflection === "string" ? value.reflection : "",
    tags: Array.isArray(value.tags)
      ? value.tags
          .filter((tag): tag is string => typeof tag === "string")
          .map((tag) => tag.trim())
          .filter(Boolean)
          .slice(0, 12)
      : [],
    createdAt: typeof value.createdAt === "string" ? value.createdAt : now,
    updatedAt: typeof value.updatedAt === "string" ? value.updatedAt : now,
    publishedAt:
      typeof value.publishedAt === "string" ? value.publishedAt : undefined,
  };
}

function inlineMarkdown(text: string): string {
  return escapeHtml(text)
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/\*(.+?)\*/g, "<em>$1</em>")
    .replace(
      /\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g,
      '<a href="$2" rel="noopener noreferrer">$1</a>',
    );
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function firstMeaningfulTag(tags: string[]): string | null {
  return tags.find((tag) => !["note", "執筆"].includes(tag.toLowerCase())) ?? null;
}

function keyPhrase(text: string): string {
  const normalized = text.replace(/\s+/g, " ").trim();
  if (!normalized) {
    return "note執筆";
  }
  return normalized.slice(0, 16);
}

function uniqueByValue(value: string, index: number, values: string[]): boolean {
  return values.indexOf(value) === index;
}

function cryptoSafeId(prefix: string): string {
  const random =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : Math.random().toString(16).slice(2);
  return `${prefix}-${random}`;
}
