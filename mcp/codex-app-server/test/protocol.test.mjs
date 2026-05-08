import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { test } from "node:test";

function startBridge() {
  const child = spawn(process.execPath, ["server.mjs"], {
    cwd: new URL("..", import.meta.url),
    env: process.env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const pending = new Map();
  let nextId = 1;

  output.on("line", (line) => {
    if (!line.trim()) {
      return;
    }
    const message = JSON.parse(line);
    const waiter = pending.get(message.id);
    if (!waiter) {
      return;
    }
    pending.delete(message.id);
    if (message.error) {
      waiter.reject(new Error(JSON.stringify(message.error)));
    } else {
      waiter.resolve(message.result);
    }
  });

  return {
    request(method, params = {}) {
      const id = nextId++;
      const message = { jsonrpc: "2.0", id, method, params };
      child.stdin.write(`${JSON.stringify(message)}\n`);
      return new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
          reject(new Error(`Timed out waiting for ${method}`));
        }, 5000);
        pending.set(id, {
          resolve(value) {
            clearTimeout(timeout);
            resolve(value);
          },
          reject(error) {
            clearTimeout(timeout);
            reject(error);
          },
        });
      });
    },
    async close() {
      child.stdin.end();
      const [code] = await once(child, "exit");
      assert.equal(code, 0);
    },
  };
}

test("MCP initialize, ping, and tools/list work without starting Codex", async () => {
  const bridge = startBridge();
  try {
    const init = await bridge.request("initialize", {
      protocolVersion: "2024-11-05",
      clientInfo: { name: "test", version: "0.0.0" },
      capabilities: {},
    });
    assert.equal(init.serverInfo.name, "codex-app-server");
    assert.equal(init.serverInfo.version, "0.1.1");

    const ping = await bridge.request("ping");
    assert.deepEqual(ping, {});

    const listed = await bridge.request("tools/list");
    assert.deepEqual(
      listed.tools.map((tool) => tool.name),
      [
        "codex_chat",
        "codex_continue",
        "codex_thread_read",
        "codex_auth_status",
        "codex_model_list",
      ],
    );
  } finally {
    await bridge.close();
  }
});
