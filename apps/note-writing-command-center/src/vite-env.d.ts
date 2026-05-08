/// <reference types="vite/client" />

import type { Article, AssistantTask } from "./shared/note";

export type CodexAskRequest = {
  task: AssistantTask;
  prompt: string;
  article: Article;
};

export type CodexAskResponse = {
  ok: boolean;
  text: string;
  source: "codex" | "local";
  error?: string;
};

export type DesktopBridge = {
  loadArticles: () => Promise<Article[]>;
  saveArticles: (articles: Article[]) => Promise<{ ok: boolean }>;
  askCodex: (request: CodexAskRequest) => Promise<CodexAskResponse>;
  codexStatus: () => Promise<{ available: boolean; detail: string }>;
};

declare global {
  interface Window {
    noteCommandCenter?: DesktopBridge;
  }
}
