import { app, BrowserWindow, ipcMain, shell } from "electron";
import path from "node:path";
import { ArticleStore } from "./articleStore";
import { CodexAppServerClient } from "./codexAppServer";
import {
  buildAssistantPrompt,
  localAssistantResponse,
  sanitizeArticle,
  type AssistantTask,
} from "../src/shared/note";

let mainWindow: BrowserWindow | null = null;
let codexClient: CodexAppServerClient | null = null;
let articleStore: ArticleStore | null = null;

function createWindow(): void {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 920,
    minWidth: 1120,
    minHeight: 760,
    title: "note執筆司令塔",
    backgroundColor: "#f5f5f4",
    titleBarStyle: "hiddenInset",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    if (url.startsWith("https://") || url.startsWith("http://")) {
      void shell.openExternal(url);
    }
    return { action: "deny" };
  });

  const devServerUrl = process.env.VITE_DEV_SERVER_URL;
  if (devServerUrl) {
    void mainWindow.loadURL(devServerUrl);
  } else {
    void mainWindow.loadFile(path.join(__dirname, "..", "..", "dist", "index.html"));
  }
}

app.whenReady().then(() => {
  articleStore = new ArticleStore(app.getPath("userData"));
  codexClient = new CodexAppServerClient(app.getVersion());
  registerIpc();
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

app.on("before-quit", () => {
  void codexClient?.stop();
});

function registerIpc(): void {
  ipcMain.handle("articles:load", async () => {
    return articleStore?.load() ?? [];
  });

  ipcMain.handle("articles:save", async (_event, articles: unknown) => {
    await articleStore?.save(articles);
    return { ok: true };
  });

  ipcMain.handle("codex:status", async () => {
    return codexClient?.status() ?? {
      available: false,
      detail: "codex app-serverは未初期化です",
    };
  });

  ipcMain.handle(
    "codex:ask",
    async (
      _event,
      request: { task?: AssistantTask; prompt?: string; article?: unknown },
    ) => {
      const task = request.task;
      const article = sanitizeArticle(request.article);
      if (!task || !article) {
        return {
          ok: false,
          source: "local",
          text: "",
          error: "assistant request payload is invalid",
        };
      }

      const prompt =
        typeof request.prompt === "string" && request.prompt.trim()
          ? request.prompt
          : buildAssistantPrompt(task, article);

      try {
        const text = await codexClient?.ask({ prompt, cwd: app.getPath("documents") });
        if (text?.trim()) {
          return { ok: true, source: "codex", text };
        }
      } catch (error) {
        return {
          ok: false,
          source: "local",
          text: localAssistantResponse(task, article),
          error: error instanceof Error ? error.message : String(error),
        };
      }

      return {
        ok: true,
        source: "local",
        text: localAssistantResponse(task, article),
      };
    },
  );
}
