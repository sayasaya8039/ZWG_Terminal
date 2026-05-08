import { describe, expect, it } from "vitest";
import {
  buildPreflightChecklist,
  createSeedArticles,
  extractOutline,
  generateTitleCandidates,
  markdownToNoteHtml,
  replaceArticle,
  sanitizeArticle,
} from "./note";

describe("note article helpers", () => {
  it("extracts h1-h3 outline with hierarchy indentation", () => {
    expect(extractOutline("# Title\n\n## A\n### B\n#### ignored")).toEqual([
      "Title",
      "  A",
      "    B",
    ]);
  });

  it("converts markdown to note-safe html and escapes raw scripts", () => {
    const html = markdownToNoteHtml(
      "# 見出し\n\n本文 **強調** <script>alert(1)</script>\n\n- one\n- two",
    );

    expect(html).toContain("<h1>見出し</h1>");
    expect(html).toContain("<strong>強調</strong>");
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).toContain("<ul><li>one</li><li>two</li></ul>");
  });

  it("builds actionable title candidates without duplicates", () => {
    const [article] = createSeedArticles();
    const titles = generateTitleCandidates(article);

    expect(titles.length).toBeGreaterThanOrEqual(4);
    expect(new Set(titles).size).toBe(titles.length);
    expect(titles.join("\n")).toContain("執筆習慣");
  });

  it("preflight checklist flags short drafts", () => {
    const short = {
      ...createSeedArticles()[0],
      title: "短い",
      markdown: "# 短い\n\n本文だけ",
      thumbnailBrief: "",
    };
    const checklist = buildPreflightChecklist(short);

    expect(checklist.some((item) => !item.passed)).toBe(true);
    expect(checklist.find((item) => item.label === "タイトル")?.passed).toBe(false);
  });

  it("replaces articles immutably", () => {
    const articles = createSeedArticles();
    const next = { ...articles[0], title: "更新後タイトル" };
    const replaced = replaceArticle(articles, next);

    expect(replaced).not.toBe(articles);
    expect(replaced[0].title).toBe("更新後タイトル");
    expect(articles[0].title).not.toBe("更新後タイトル");
  });

  it("sanitizes persisted article records", () => {
    const sanitized = sanitizeArticle({
      id: "x",
      title: "t",
      status: "draft",
      markdown: "body",
      tags: [" note ", 3, "執筆"],
    });

    expect(sanitized?.tags).toEqual(["note", "執筆"]);
    expect(sanitizeArticle({ id: "x", status: "broken" })).toBeNull();
  });
});
