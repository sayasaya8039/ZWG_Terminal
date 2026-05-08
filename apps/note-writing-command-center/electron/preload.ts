import { contextBridge, ipcRenderer } from "electron";
import type { Article, AssistantTask } from "../src/shared/note";

contextBridge.exposeInMainWorld("noteCommandCenter", {
  loadArticles: () => ipcRenderer.invoke("articles:load") as Promise<Article[]>,
  saveArticles: (articles: Article[]) =>
    ipcRenderer.invoke("articles:save", articles) as Promise<{ ok: boolean }>,
  askCodex: (request: { task: AssistantTask; prompt: string; article: Article }) =>
    ipcRenderer.invoke("codex:ask", request) as Promise<{
      ok: boolean;
      text: string;
      source: "codex" | "local";
      error?: string;
    }>,
  codexStatus: () =>
    ipcRenderer.invoke("codex:status") as Promise<{
      available: boolean;
      detail: string;
    }>,
});
