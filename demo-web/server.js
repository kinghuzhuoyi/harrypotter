import http from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import os from "node:os";

const ROOT = fileURLToPath(new URL(".", import.meta.url));
const PUBLIC_DIR = join(ROOT, "public");
const HOST = process.env.HOST || "0.0.0.0";
const PORT = Number(process.env.PORT || 4173);

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url || "/", `http://${request.headers.host || "localhost"}`);
  if (request.method !== "GET" && request.method !== "HEAD") return text(response, 405, "Method not allowed");
  return serveStatic(url.pathname, response, request.method === "HEAD");
});

server.listen(PORT, HOST, () => {
  console.log(`Riddle whiteboard demo: http://localhost:${PORT}`);
  for (const address of lanAddresses()) console.log(`iPad / LAN:             http://${address}:${PORT}`);
});

async function serveStatic(pathname, response, headOnly) {
  const requested = pathname === "/" ? "index.html" : pathname.replace(/^\//, "");
  const filePath = join(PUBLIC_DIR, normalize(requested).replace(/^(\.\.[/\\])+/, ""));
  if (!filePath.startsWith(PUBLIC_DIR)) return text(response, 404, "Not found");
  try {
    const file = await readFile(filePath);
    const types = { ".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8" };
    response.writeHead(200, { "Content-Type": types[extname(filePath)] || "application/octet-stream", "Cache-Control": "no-store" });
    response.end(headOnly ? undefined : file);
  } catch { text(response, 404, "Not found"); }
}

function text(response, status, value) { response.writeHead(status, { "Content-Type": "text/plain; charset=utf-8" }); response.end(value); }
function lanAddresses() { return Object.values(os.networkInterfaces()).flat().filter((item) => item?.family === "IPv4" && !item.internal).map((item) => item.address); }
