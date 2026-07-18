import http from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import os from "node:os";
import { parseModelTurn, postWithModelFallback, userFacingApiError } from "./oracle-client.js";

const ROOT = fileURLToPath(new URL(".", import.meta.url));
const PUBLIC_DIR = join(ROOT, "public");
const DATA_DIR = join(ROOT, "data");
const IMAGE_DIR = join(DATA_DIR, "images");
const MEMORY_FILE = join(DATA_DIR, "memories.json");
const MAX_BODY = 12 * 1024 * 1024;

await loadEnv(join(ROOT, ".env"));
const HOST = process.env.HOST || "0.0.0.0";
const PORT = Number(process.env.PORT || 4173);
await mkdir(IMAGE_DIR, { recursive: true });
if (!existsSync(MEMORY_FILE)) await writeJson(MEMORY_FILE, []);

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url || "/", `http://${request.headers.host || "localhost"}`);
    if (request.method === "GET" && url.pathname === "/api/config") {
      return json(response, 200, {
        configured: Boolean(process.env.RIDDLE_OPENAI_KEY),
        provider: "zhipu",
        base: process.env.RIDDLE_OPENAI_BASE || "https://open.bigmodel.cn/api/paas/v4",
        model: process.env.RIDDLE_OPENAI_MODEL || "glm-4.6v-flash"
      });
    }
    if (request.method === "GET" && url.pathname === "/api/memories") {
      return json(response, 200, { memories: (await readMemories()).reverse() });
    }
    if (request.method === "POST" && url.pathname === "/api/ask") {
      const body = await readJsonBody(request);
      if (!body.imageDataUrl?.startsWith("data:image/png;base64,")) return json(response, 400, { error: "缺少有效的白板 PNG。" });
      if (!process.env.RIDDLE_OPENAI_KEY) return json(response, 503, { error: "尚未配置 RIDDLE_OPENAI_KEY，请先创建 .env。" });

      const memories = await readMemories();
      const turn = await askOracle(body.imageDataUrl, memories);
      const id = `${Date.now()}-${crypto.randomUUID().slice(0, 8)}`;
      const imageName = `${id}.png`;
      await writeFile(join(IMAGE_DIR, imageName), Buffer.from(body.imageDataUrl.split(",", 2)[1], "base64"));
      const memory = { id, createdAt: new Date().toISOString(), transcript: turn.transcript, reply: turn.reply, image: `images/${imageName}` };
      memories.push(memory);
      await writeJson(MEMORY_FILE, memories.slice(-400));
      return json(response, 200, { memory });
    }
    if (request.method === "GET" && url.pathname.startsWith("/memory-images/")) {
      const name = url.pathname.slice(15);
      if (!/^[a-zA-Z0-9-]+\.png$/.test(name)) return text(response, 404, "Not found");
      const file = await readFile(join(IMAGE_DIR, name));
      response.writeHead(200, { "Content-Type": "image/png", "Cache-Control": "no-store" });
      return response.end(file);
    }
    return serveStatic(url.pathname, response);
  } catch (error) {
    console.error(error);
    return json(response, error.statusCode || 500, { error: error.message || "服务器发生错误。" });
  }
});

server.listen(PORT, HOST, () => {
  console.log(`Riddle whiteboard demo: http://localhost:${PORT}`);
  for (const address of lanAddresses()) console.log(`iPad / LAN:             http://${address}:${PORT}`);
});

async function askOracle(imageDataUrl, memories) {
  const base = (process.env.RIDDLE_OPENAI_BASE || "https://open.bigmodel.cn/api/paas/v4").replace(/\/$/, "");
  const model = process.env.RIDDLE_OPENAI_MODEL || "glm-4.6v-flash";
  const fallbackModels = (process.env.RIDDLE_MODEL_FALLBACKS || "glm-4.1v-thinking-flash,glm-4v-flash").split(",");
  const turns = Math.max(0, Number(process.env.MEMORY_TURNS || 6));
  const history = memories.slice(-turns).map(({ transcript, reply, createdAt }) => ({ written: transcript, answer: reply, date: createdAt }));
  const system = [
    "You are the quiet spirit of a handwritten diary.",
    "Read the handwriting in the supplied whiteboard image and answer in the same language.",
    "Be warm, concise and slightly mysterious. Reply in one to three sentences.",
    "Return ONLY valid JSON with string fields transcript and reply.",
    "transcript is a faithful transcription of the current handwriting; reply is your answer.",
    "Never mention images, OCR, models or AI. If unreadable, say the ink is unclear."
  ].join(" ");
  const userText = history.length ? `Recent diary memories, oldest first:\n${JSON.stringify(history)}\n\nRead and answer the new writing.` : "Read and answer the new writing.";
  const { response: apiResponse, payload, model: usedModel } = await postWithModelFallback({
    base,
    key: process.env.RIDDLE_OPENAI_KEY,
    models: [model, ...fallbackModels],
    body: {
      temperature: 0.7,
      max_tokens: 500,
      messages: [
        { role: "system", content: system },
        { role: "user", content: [{ type: "text", text: userText }, { type: "image_url", image_url: { url: imageDataUrl } }] }
      ]
    }
  });
  if (!apiResponse.ok) {
    const error = new Error(userFacingApiError(apiResponse.status, payload.error));
    error.statusCode = 502;
    throw error;
  }
  const raw = payload.choices?.[0]?.message?.content;
  if (!raw) throw new Error("模型没有返回回答。");
  if (usedModel !== model) console.log(`riddle: model fallback ${model} -> ${usedModel}`);
  return parseModelTurn(raw);
}

async function serveStatic(pathname, response) {
  const requested = pathname === "/" ? "index.html" : pathname.replace(/^\//, "");
  const filePath = join(PUBLIC_DIR, normalize(requested).replace(/^(\.\.[/\\])+/, ""));
  if (!filePath.startsWith(PUBLIC_DIR)) return text(response, 404, "Not found");
  try {
    const file = await readFile(filePath);
    const types = { ".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8" };
    response.writeHead(200, { "Content-Type": types[extname(filePath)] || "application/octet-stream", "Cache-Control": "no-store" });
    response.end(file);
  } catch { text(response, 404, "Not found"); }
}

async function readJsonBody(request) {
  const chunks = [];
  let length = 0;
  for await (const chunk of request) {
    length += chunk.length;
    if (length > MAX_BODY) { const error = new Error("请求内容过大。"); error.statusCode = 413; throw error; }
    chunks.push(chunk);
  }
  try { return JSON.parse(Buffer.concat(chunks).toString("utf8")); }
  catch { const error = new Error("请求 JSON 无效。"); error.statusCode = 400; throw error; }
}

async function readMemories() {
  try { const value = JSON.parse(await readFile(MEMORY_FILE, "utf8")); return Array.isArray(value) ? value : []; }
  catch { return []; }
}
async function writeJson(path, value) { await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, "utf8"); }
async function loadEnv(path) {
  if (!existsSync(path)) return;
  for (const line of (await readFile(path, "utf8")).split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const index = trimmed.indexOf("=");
    if (index < 1) continue;
    const key = trimmed.slice(0, index).trim();
    const value = trimmed.slice(index + 1).trim().replace(/^(['"])(.*)\1$/, "$2");
    if (!(key in process.env)) process.env[key] = value;
  }
}
function json(response, status, value) { response.writeHead(status, { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store" }); response.end(JSON.stringify(value)); }
function text(response, status, value) { response.writeHead(status, { "Content-Type": "text/plain; charset=utf-8" }); response.end(value); }
function lanAddresses() { return Object.values(os.networkInterfaces()).flat().filter((item) => item?.family === "IPv4" && !item.internal).map((item) => item.address); }
