import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import os from "node:os";
import path from "node:path";

type JsonRpcMessage = {
  id?: number | string;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code?: number; message?: string; data?: unknown };
};

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
};

type PendingTurn = {
  threadId: string;
  turnId: string;
  text: string;
  resolve: (text: string) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
};

export type CodexAskOptions = {
  prompt: string;
  cwd?: string;
};

export class CodexAppServerClient extends EventEmitter {
  private child: ChildProcessWithoutNullStreams | null = null;
  private nextId = 1;
  private buffer = "";
  private pendingRequests = new Map<number, PendingRequest>();
  private pendingTurns = new Map<string, PendingTurn>();
  private initializePromise: Promise<void> | null = null;
  private lastError = "";

  constructor(private readonly appVersion: string) {
    super();
  }

  status(): { available: boolean; detail: string } {
    if (this.child && !this.child.killed) {
      return { available: true, detail: "codex app-server接続中" };
    }
    return {
      available: false,
      detail: this.lastError || "codex app-serverは未接続です",
    };
  }

  async ask(options: CodexAskOptions): Promise<string> {
    await this.ensureInitialized();
    const cwd = options.cwd ?? os.homedir();
    const thread = await this.request("thread/start", {
      cwd,
      approvalPolicy: "never",
      sandbox: "read-only",
      serviceName: "note_writing_command_center",
      personality: "pragmatic",
      ephemeral: true,
      threadSource: "appServer",
      experimentalRawEvents: false,
      persistExtendedHistory: false,
    });

    const threadId = readNestedString(thread, ["thread", "id"]);
    if (!threadId) {
      throw new Error("codex app-server thread/start did not return a thread id");
    }

    const turn = await this.request("turn/start", {
      threadId,
      input: [{ type: "text", text: options.prompt, text_elements: [] }],
      cwd,
      approvalPolicy: "never",
      sandboxPolicy: { type: "readOnly", networkAccess: false },
      personality: "pragmatic",
    });

    const turnId = readNestedString(turn, ["turn", "id"]);
    if (!turnId) {
      throw new Error("codex app-server turn/start did not return a turn id");
    }

    return this.waitForTurn(threadId, turnId);
  }

  async stop(): Promise<void> {
    for (const request of this.pendingRequests.values()) {
      clearTimeout(request.timer);
      request.reject(new Error("codex app-server stopped"));
    }
    this.pendingRequests.clear();

    for (const turn of this.pendingTurns.values()) {
      clearTimeout(turn.timer);
      turn.reject(new Error("codex app-server stopped"));
    }
    this.pendingTurns.clear();

    if (this.child && !this.child.killed) {
      this.child.kill();
    }
    this.child = null;
    this.initializePromise = null;
  }

  private async ensureInitialized(): Promise<void> {
    if (this.initializePromise) {
      return this.initializePromise;
    }

    this.initializePromise = this.startAndInitialize();
    try {
      await this.initializePromise;
    } catch (error) {
      this.initializePromise = null;
      throw error;
    }
  }

  private async startAndInitialize(): Promise<void> {
    if (!this.child || this.child.killed) {
      this.child = spawn("codex", ["app-server", "--listen", "stdio://"], {
        cwd: path.resolve(process.cwd()),
        windowsHide: true,
      });
      this.child.stdout.setEncoding("utf8");
      this.child.stderr.setEncoding("utf8");
      this.child.stdout.on("data", (chunk: string) => this.readStdout(chunk));
      this.child.stderr.on("data", (chunk: string) => {
        this.lastError = chunk.trim().slice(-600);
        this.emit("stderr", this.lastError);
      });
      this.child.on("exit", (code) => {
        this.lastError = `codex app-serverが終了しました: ${code ?? "unknown"}`;
        this.rejectAll(new Error(this.lastError));
        this.child = null;
        this.initializePromise = null;
      });
      this.child.on("error", (error) => {
        this.lastError = error.message;
        this.rejectAll(error);
        this.child = null;
        this.initializePromise = null;
      });
    }

    await this.request("initialize", {
      clientInfo: {
        name: "note_writing_command_center",
        title: "note執筆司令塔",
        version: this.appVersion,
      },
      capabilities: {
        experimentalApi: true,
      },
    });
    this.notify("initialized", {});
  }

  private request(method: string, params: unknown, timeoutMs = 45_000): Promise<unknown> {
    const id = this.nextId++;
    const message = { id, method, params };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      this.pendingRequests.set(id, { resolve, reject, timer });
      this.write(message);
    });
  }

  private notify(method: string, params: unknown): void {
    this.write({ method, params });
  }

  private write(message: JsonRpcMessage): void {
    if (!this.child || !this.child.stdin.writable) {
      throw new Error("codex app-server process is not writable");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  private readStdout(chunk: string): void {
    this.buffer += chunk;
    let newline = this.buffer.indexOf("\n");
    while (newline >= 0) {
      const raw = this.buffer.slice(0, newline).trim();
      this.buffer = this.buffer.slice(newline + 1);
      if (raw) {
        this.handleMessage(raw);
      }
      newline = this.buffer.indexOf("\n");
    }
  }

  private handleMessage(raw: string): void {
    let message: JsonRpcMessage;
    try {
      message = JSON.parse(raw) as JsonRpcMessage;
    } catch {
      return;
    }

    if (typeof message.id === "number" && !message.method) {
      const pending = this.pendingRequests.get(message.id);
      if (!pending) {
        return;
      }
      clearTimeout(pending.timer);
      this.pendingRequests.delete(message.id);
      if (message.error) {
        pending.reject(new Error(message.error.message ?? "codex app-server request failed"));
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (message.method) {
      this.handleNotification(message.method, message.params);
    }
  }

  private handleNotification(method: string, params: unknown): void {
    if (method === "item/agentMessage/delta") {
      const threadId = readNestedString(params, ["threadId"]);
      const turnId = readNestedString(params, ["turnId"]);
      const delta = readNestedString(params, ["delta"]);
      if (threadId && turnId && delta) {
        const pending = this.pendingTurns.get(turnKey(threadId, turnId));
        if (pending) {
          pending.text += delta;
        }
      }
      return;
    }

    if (method === "item/completed") {
      const threadId = readNestedString(params, ["threadId"]);
      const turnId = readNestedString(params, ["turnId"]);
      const itemType = readNestedString(params, ["item", "type"]);
      const text = readNestedString(params, ["item", "text"]);
      if (threadId && turnId && itemType === "agentMessage" && text) {
        const pending = this.pendingTurns.get(turnKey(threadId, turnId));
        if (pending && pending.text.trim().length === 0) {
          pending.text = text;
        }
      }
      return;
    }

    if (method === "turn/completed") {
      const threadId = readNestedString(params, ["threadId"]);
      const turnId = readNestedString(params, ["turn", "id"]);
      if (!threadId || !turnId) {
        return;
      }
      const key = turnKey(threadId, turnId);
      const pending = this.pendingTurns.get(key);
      if (!pending) {
        return;
      }
      clearTimeout(pending.timer);
      this.pendingTurns.delete(key);
      const text = pending.text.trim();
      if (text) {
        pending.resolve(text);
      } else {
        pending.reject(new Error("codex app-server returned an empty assistant response"));
      }
    }
  }

  private waitForTurn(threadId: string, turnId: string, timeoutMs = 120_000): Promise<string> {
    const key = turnKey(threadId, turnId);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingTurns.delete(key);
        reject(new Error("Codexの応答がタイムアウトしました"));
      }, timeoutMs);
      this.pendingTurns.set(key, {
        threadId,
        turnId,
        text: "",
        resolve,
        reject,
        timer,
      });
    });
  }

  private rejectAll(error: Error): void {
    for (const request of this.pendingRequests.values()) {
      clearTimeout(request.timer);
      request.reject(error);
    }
    this.pendingRequests.clear();
    for (const turn of this.pendingTurns.values()) {
      clearTimeout(turn.timer);
      turn.reject(error);
    }
    this.pendingTurns.clear();
  }
}

function turnKey(threadId: string, turnId: string): string {
  return `${threadId}:${turnId}`;
}

function readNestedString(value: unknown, pathParts: string[]): string | null {
  let cursor = value;
  for (const part of pathParts) {
    if (!cursor || typeof cursor !== "object" || !(part in cursor)) {
      return null;
    }
    cursor = (cursor as Record<string, unknown>)[part];
  }
  return typeof cursor === "string" ? cursor : null;
}
