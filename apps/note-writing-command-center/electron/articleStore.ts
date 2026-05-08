import fs from "node:fs/promises";
import path from "node:path";
import type { Article } from "../src/shared/note";
import { createSeedArticles, sanitizeArticle } from "../src/shared/note";

type PersistedArticles = {
  version: 1;
  articles: Article[];
};

export class ArticleStore {
  constructor(private readonly userDataPath: string) {}

  async load(): Promise<Article[]> {
    const filePath = this.filePath();
    try {
      const raw = await fs.readFile(filePath, "utf8");
      const parsed = JSON.parse(raw) as Partial<PersistedArticles>;
      if (!Array.isArray(parsed.articles)) {
        return createSeedArticles();
      }
      const articles = parsed.articles
        .map((article) => sanitizeArticle(article))
        .filter((article): article is Article => Boolean(article));
      return articles.length > 0 ? articles : createSeedArticles();
    } catch (error) {
      if (isNotFound(error)) {
        const seed = createSeedArticles();
        await this.save(seed);
        return seed;
      }
      throw error;
    }
  }

  async save(input: unknown): Promise<void> {
    if (!Array.isArray(input)) {
      throw new Error("articles payload must be an array");
    }
    const articles = input
      .map((article) => sanitizeArticle(article))
      .filter((article): article is Article => Boolean(article))
      .slice(0, 500);
    await fs.mkdir(this.userDataPath, { recursive: true });
    const payload: PersistedArticles = {
      version: 1,
      articles,
    };
    await fs.writeFile(this.filePath(), `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  }

  private filePath(): string {
    return path.join(this.userDataPath, "note-articles.json");
  }
}

function isNotFound(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { code?: string }).code === "ENOENT"
  );
}
