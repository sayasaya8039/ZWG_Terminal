import {
  ClipboardCheck,
  Code2,
  Copy,
  FileText,
  Image,
  LayoutList,
  Plus,
  RefreshCcw,
  Search,
  Send,
  Sparkles,
} from "lucide-react";
import { useEffect, useMemo, useState, type ChangeEvent } from "react";
import {
  ARTICLE_STATUSES,
  ASSISTANT_TASK_LABELS,
  EDITOR_MODE_LABELS,
  STATUS_LABELS,
  addArticle,
  buildAssistantPrompt,
  buildPreflightChecklist,
  buildSuggestedOutline,
  createEmptyArticle,
  createSeedArticles,
  extractOutline,
  localAssistantResponse,
  markdownToNoteHtml,
  replaceArticle,
  summarizeStatuses,
  type Article,
  type ArticleStatus,
  type AssistantTask,
  type EditorMode,
} from "./shared/note";

const ASSISTANT_TASKS: Array<{
  task: AssistantTask;
  icon: typeof Sparkles;
}> = [
  { task: "outline", icon: LayoutList },
  { task: "titles", icon: Sparkles },
  { task: "readability", icon: FileText },
  { task: "thumbnail", icon: Image },
  { task: "preflight", icon: ClipboardCheck },
  { task: "format", icon: Code2 },
];

export function App() {
  const [articles, setArticles] = useState<Article[]>(() => createSeedArticles());
  const [selectedId, setSelectedId] = useState<string>(() => createSeedArticles()[0].id);
  const [statusFilter, setStatusFilter] = useState<ArticleStatus | "all">("all");
  const [query, setQuery] = useState("");
  const [editorMode, setEditorMode] = useState<EditorMode>("markdown");
  const [assistantTask, setAssistantTask] = useState<AssistantTask>("outline");
  const [assistantText, setAssistantText] = useState(() =>
    localAssistantResponse("outline", createSeedArticles()[0]),
  );
  const [assistantSource, setAssistantSource] = useState<"codex" | "local">("local");
  const [assistantBusy, setAssistantBusy] = useState(false);
  const [assistantError, setAssistantError] = useState<string | null>(null);
  const [codexStatus, setCodexStatus] = useState("codex app-server確認中");
  const [loadedFromDesktop, setLoadedFromDesktop] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const bridge = window.noteCommandCenter;
      if (!bridge) {
        setLoadedFromDesktop(true);
        setCodexStatus("ブラウザプレビュー: ローカル支援");
        return;
      }
      const [loadedArticles, status] = await Promise.all([
        bridge.loadArticles(),
        bridge.codexStatus(),
      ]);
      if (cancelled) {
        return;
      }
      setArticles(loadedArticles);
      setSelectedId(loadedArticles[0]?.id ?? "");
      setCodexStatus(status.detail);
      setLoadedFromDesktop(true);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!loadedFromDesktop || !window.noteCommandCenter) {
      return;
    }
    const timeout = window.setTimeout(() => {
      void window.noteCommandCenter?.saveArticles(articles);
    }, 400);
    return () => window.clearTimeout(timeout);
  }, [articles, loadedFromDesktop]);

  const selectedArticle = useMemo(
    () => articles.find((article) => article.id === selectedId) ?? articles[0],
    [articles, selectedId],
  );

  const filteredArticles = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    return articles.filter((article) => {
      const statusMatches = statusFilter === "all" || article.status === statusFilter;
      const queryMatches =
        !normalizedQuery ||
        [article.title, article.ideaMemo, article.markdown, article.tags.join(" ")]
          .join("\n")
          .toLowerCase()
          .includes(normalizedQuery);
      return statusMatches && queryMatches;
    });
  }, [articles, query, statusFilter]);

  const statusSummary = useMemo(() => summarizeStatuses(articles), [articles]);
  const noteHtml = useMemo(
    () => markdownToNoteHtml(selectedArticle?.markdown ?? ""),
    [selectedArticle?.markdown],
  );
  const outline = useMemo(
    () => extractOutline(selectedArticle?.markdown ?? ""),
    [selectedArticle?.markdown],
  );
  const checklist = useMemo(
    () => buildPreflightChecklist(selectedArticle),
    [selectedArticle],
  );
  const readiness = Math.round(
    (checklist.filter((item) => item.passed).length / checklist.length) * 100,
  );

  const updateSelected = (patch: Partial<Article>) => {
    if (!selectedArticle) {
      return;
    }
    setArticles((current) => replaceArticle(current, { ...selectedArticle, ...patch }));
  };

  const createArticle = () => {
    const article = createEmptyArticle();
    setArticles((current) => addArticle(current, article));
    setSelectedId(article.id);
    setStatusFilter("all");
    setEditorMode("markdown");
    setAssistantTask("outline");
    setAssistantText(localAssistantResponse("outline", article));
    setAssistantSource("local");
  };

  const runAssistant = async (task: AssistantTask) => {
    if (!selectedArticle) {
      return;
    }
    setAssistantTask(task);
    setAssistantBusy(true);
    setAssistantError(null);
    const prompt = buildAssistantPrompt(task, selectedArticle);
    try {
      const bridge = window.noteCommandCenter;
      if (!bridge) {
        setAssistantText(localAssistantResponse(task, selectedArticle));
        setAssistantSource("local");
        return;
      }
      const result = await bridge.askCodex({ task, prompt, article: selectedArticle });
      setAssistantText(result.text || localAssistantResponse(task, selectedArticle));
      setAssistantSource(result.source);
      setAssistantError(result.error ?? null);
      const status = await bridge.codexStatus();
      setCodexStatus(status.detail);
    } finally {
      setAssistantBusy(false);
    }
  };

  const applySuggestedOutline = () => {
    if (!selectedArticle) {
      return;
    }
    const suggested = buildSuggestedOutline(selectedArticle);
    const hasBody = selectedArticle.markdown.trim().length > 0;
    updateSelected({
      markdown: hasBody ? `${selectedArticle.markdown.trim()}\n\n${suggested}` : suggested,
      status: selectedArticle.status === "idea" ? "draft" : selectedArticle.status,
    });
    setEditorMode("markdown");
  };

  const copyNoteHtml = async () => {
    await navigator.clipboard.writeText(noteHtml);
  };

  if (!selectedArticle) {
    return (
      <main className="app-shell">
        <button className="primary-command" onClick={createArticle}>
          <Plus size={16} />
          新規記事を作成
        </button>
      </main>
    );
  }

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand-row">
          <div>
            <h1>note執筆司令塔</h1>
            <p>書く前と貼る直前を整える</p>
          </div>
          <button className="icon-command" onClick={createArticle} title="新規記事">
            <Plus size={17} />
          </button>
        </div>

        <div className="search-box">
          <Search size={15} />
          <input
            aria-label="記事検索"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="タイトル、タグ、本文を検索"
          />
        </div>

        <nav className="status-list" aria-label="記事状態">
          <button
            className={statusFilter === "all" ? "active" : ""}
            onClick={() => setStatusFilter("all")}
          >
            すべて
            <span>{articles.length}</span>
          </button>
          {ARTICLE_STATUSES.map((status) => (
            <button
              key={status}
              className={statusFilter === status ? "active" : ""}
              onClick={() => setStatusFilter(status)}
            >
              {STATUS_LABELS[status]}
              <span>{statusSummary[status]}</span>
            </button>
          ))}
        </nav>

        <div className="article-list">
          {filteredArticles.map((article) => (
            <button
              key={article.id}
              className={`article-item ${article.id === selectedArticle.id ? "selected" : ""}`}
              onClick={() => setSelectedId(article.id)}
            >
              <span className={`status-dot ${article.status}`} />
              <strong>{article.title}</strong>
              <small>{article.tags.join(" / ") || "タグなし"}</small>
            </button>
          ))}
        </div>
      </aside>

      <section className="editor-pane">
        <header className="editor-header">
          <div className="title-group">
            <input
              className="title-input"
              value={selectedArticle.title}
              onChange={(event) => updateSelected({ title: event.target.value })}
              aria-label="記事タイトル"
            />
            <div className="meta-row">
              <select
                value={selectedArticle.status}
                onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                  updateSelected({ status: event.target.value as ArticleStatus })
                }
              >
                {ARTICLE_STATUSES.map((status) => (
                  <option key={status} value={status}>
                    {STATUS_LABELS[status]}
                  </option>
                ))}
              </select>
              <input
                value={selectedArticle.tags.join(", ")}
                onChange={(event) =>
                  updateSelected({
                    tags: event.target.value
                      .split(/[,\u3001]/)
                      .map((tag) => tag.trim())
                      .filter(Boolean),
                  })
                }
                aria-label="タグ"
                placeholder="タグをカンマ区切りで入力"
              />
            </div>
          </div>
          <div className="readiness-meter">
            <span>投稿準備</span>
            <strong>{readiness}%</strong>
          </div>
        </header>

        <div className="workbench">
          <div className="idea-strip">
            <label>
              ネタメモ
              <textarea
                value={selectedArticle.ideaMemo}
                onChange={(event) => updateSelected({ ideaMemo: event.target.value })}
              />
            </label>
            <label>
              サムネイル指示
              <textarea
                value={selectedArticle.thumbnailBrief}
                onChange={(event) => updateSelected({ thumbnailBrief: event.target.value })}
              />
            </label>
          </div>

          <div className="mode-strip">
            {(Object.keys(EDITOR_MODE_LABELS) as EditorMode[]).map((mode) => (
              <button
                key={mode}
                className={editorMode === mode ? "active" : ""}
                onClick={() => setEditorMode(mode)}
              >
                {mode === "markdown" && <FileText size={15} />}
                {mode === "preview" && <RefreshCcw size={15} />}
                {mode === "noteHtml" && <Code2 size={15} />}
                {EDITOR_MODE_LABELS[mode]}
              </button>
            ))}
            <button className="subtle-command" onClick={applySuggestedOutline}>
              <LayoutList size={15} />
              構成テンプレを追加
            </button>
            <button className="subtle-command" onClick={copyNoteHtml}>
              <Copy size={15} />
              HTMLコピー
            </button>
          </div>

          <div className="editor-surface">
            {editorMode === "markdown" && (
              <textarea
                className="markdown-editor"
                value={selectedArticle.markdown}
                onChange={(event) => updateSelected({ markdown: event.target.value })}
                spellCheck={false}
              />
            )}
            {editorMode === "preview" && (
              <article
                className="preview-document"
                dangerouslySetInnerHTML={{ __html: noteHtml }}
              />
            )}
            {editorMode === "noteHtml" && (
              <textarea className="markdown-editor html-output" value={noteHtml} readOnly />
            )}
          </div>
        </div>
      </section>

      <aside className="assistant-pane">
        <div className="assistant-status">
          <div>
            <h2>Codexアシスタント</h2>
            <p>{codexStatus}</p>
          </div>
          <span className={`source-pill ${assistantSource}`}>{assistantSource}</span>
        </div>

        <div className="assistant-actions">
          {ASSISTANT_TASKS.map(({ task, icon: Icon }) => (
            <button
              key={task}
              className={assistantTask === task ? "active" : ""}
              onClick={() => void runAssistant(task)}
              disabled={assistantBusy}
            >
              <Icon size={15} />
              {ASSISTANT_TASK_LABELS[task]}
            </button>
          ))}
        </div>

        <section className="assistant-output">
          <div className="assistant-output-title">
            <span>{ASSISTANT_TASK_LABELS[assistantTask]}</span>
            {assistantBusy && <small>生成中</small>}
          </div>
          <pre>{assistantText}</pre>
          {assistantError && <p className="assistant-error">{assistantError}</p>}
        </section>

        <section className="outline-panel">
          <div className="panel-title">
            <LayoutList size={15} />
            見出し構成
          </div>
          {outline.length > 0 ? (
            <ol>
              {outline.map((item, index) => (
                <li key={`${item}-${index}`}>{item}</li>
              ))}
            </ol>
          ) : (
            <p>見出しがまだありません。</p>
          )}
        </section>

        <section className="check-panel">
          <div className="panel-title">
            <ClipboardCheck size={15} />
            投稿前チェック
          </div>
          {checklist.map((item) => (
            <div key={item.label} className="check-row">
              <span className={item.passed ? "passed" : "warn"}>
                {item.passed ? "OK" : "確認"}
              </span>
              <div>
                <strong>{item.label}</strong>
                <small>{item.detail}</small>
              </div>
            </div>
          ))}
        </section>

        <section className="reflection-panel">
          <div className="panel-title">
            <Send size={15} />
            公開後の振り返り
          </div>
          <textarea
            value={selectedArticle.reflection}
            onChange={(event) => updateSelected({ reflection: event.target.value })}
            placeholder="公開後の反応、数字、次の仮説を残す"
          />
        </section>
      </aside>
    </main>
  );
}
