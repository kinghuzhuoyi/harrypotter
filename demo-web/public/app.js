import { askDiary, getSupabase, listMemories } from "./supabase-client.js";

const $ = (selector) => document.querySelector(selector);
const canvas = $("#paperCanvas");
const ctx = canvas.getContext("2d", { alpha: false });
const hint = $("#paperHint");
const send = $("#sendButton");
const clear = $("#clearButton");
const status = $("#statusText");
const answerCard = $("#answerCard");
const answer = $("#answerText");
const transcript = $("#transcriptText");
const sending = $("#sendingCard");
const memoryPanel = $("#memoryPanel");
const memoryList = $("#memoryList");
const scrim = $("#scrim");
let drawing = false, hasInk = false, busy = false, cloudReady = false;
let pointer = null, last = null, cssWidth = 0, cssHeight = 0;

new ResizeObserver(resizeCanvas).observe(canvas);
canvas.addEventListener("pointerdown", start);
canvas.addEventListener("pointermove", move);
canvas.addEventListener("pointerup", end);
canvas.addEventListener("pointercancel", end);
canvas.addEventListener("contextmenu", (event) => event.preventDefault());
send.addEventListener("click", submit);
clear.addEventListener("click", clearCanvas);
$("#continueButton").addEventListener("click", returnToPaper);
$("#memoryButton").addEventListener("click", openMemories);
$("#closeMemoryButton").addEventListener("click", closeMemories);
scrim.addEventListener("click", closeMemories);
addEventListener("keydown", (event) => { if (event.key === "Escape") closeMemories(); });
initializeCloud();

function resizeCanvas() {
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const snapshot = hasInk ? canvas.toDataURL() : null;
  const ratio = Math.min(devicePixelRatio || 1, 2);
  cssWidth = rect.width; cssHeight = rect.height;
  canvas.width = Math.round(rect.width * ratio); canvas.height = Math.round(rect.height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.fillStyle = "#fbf8ef"; ctx.fillRect(0, 0, rect.width, rect.height);
  ctx.lineCap = "round"; ctx.lineJoin = "round";
  if (snapshot) { const image = new Image(); image.onload = () => ctx.drawImage(image, 0, 0, cssWidth, cssHeight); image.src = snapshot; }
}
function start(event) {
  if (busy || !answerCard.hidden || event.button > 0) return;
  event.preventDefault(); pointer = event.pointerId; canvas.setPointerCapture(pointer); drawing = true; last = point(event); dot(last); markInk();
}
function move(event) {
  if (!drawing || event.pointerId !== pointer) return;
  event.preventDefault();
  for (const item of event.getCoalescedEvents?.() || [event]) { const next = point(item); line(last, next); last = next; }
  markInk();
}
function end(event) {
  if (event.pointerId !== pointer) return;
  drawing = false; pointer = null; last = null;
  if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
}
function point(event) {
  const rect = canvas.getBoundingClientRect();
  const pressure = event.pointerType === "mouse" ? 0.42 : (event.pressure || 0.42);
  return { x: event.clientX - rect.left, y: event.clientY - rect.top, width: 1.8 + pressure * 5.2 };
}
function dot(value) { ctx.fillStyle = "#211f1b"; ctx.beginPath(); ctx.arc(value.x, value.y, value.width / 2, 0, Math.PI * 2); ctx.fill(); }
function line(from, to) { ctx.strokeStyle = "#211f1b"; ctx.lineWidth = (from.width + to.width) / 2; ctx.beginPath(); ctx.moveTo(from.x, from.y); ctx.lineTo(to.x, to.y); ctx.stroke(); }
function markInk() { hasInk = true; hint.hidden = true; send.disabled = !cloudReady; clear.disabled = false; status.textContent = cloudReady ? "纸上已有内容" : "正在连接云端"; }
function clearCanvas() { ctx.fillStyle = "#fbf8ef"; ctx.fillRect(0, 0, cssWidth, cssHeight); hasInk = false; hint.hidden = false; send.disabled = true; clear.disabled = true; status.textContent = cloudReady ? "准备书写" : "云端尚未就绪"; }

async function submit() {
  if (!hasInk || busy || !cloudReady) return;
  busy = true; const imageDataUrl = exportPng(); clearCanvas(); sending.hidden = false; status.textContent = "纸张正在阅读";
  try { const memory = await askDiary(imageDataUrl); showAnswer(memory); await loadMemories(); }
  catch (error) { showAnswer({ transcript: "", reply: `纸张没有听清：${error.message}` }); }
  finally { busy = false; sending.hidden = true; }
}
function exportPng() {
  const output = document.createElement("canvas"); const scale = Math.min(1, 1280 / canvas.width);
  output.width = Math.round(canvas.width * scale); output.height = Math.round(canvas.height * scale);
  const out = output.getContext("2d", { alpha: false }); out.fillStyle = "#fbf8ef"; out.fillRect(0, 0, output.width, output.height); out.drawImage(canvas, 0, 0, output.width, output.height);
  return output.toDataURL("image/png");
}
function showAnswer(memory) { answer.textContent = memory.reply; transcript.textContent = memory.transcript || "未能可靠辨认"; answerCard.hidden = false; status.textContent = "纸张已经回应"; }
function returnToPaper() { answerCard.hidden = true; status.textContent = "准备书写"; hint.hidden = false; }

async function initializeCloud() {
  send.disabled = true; status.textContent = "正在连接云端";
  try { await getSupabase(); cloudReady = true; status.textContent = "准备书写"; send.disabled = !hasInk; await loadMemories(); }
  catch (error) { status.textContent = error.message; memoryList.innerHTML = '<p class="memory-empty">云端尚未配置</p>'; }
}
async function loadMemories() {
  try { renderMemories(await listMemories()); }
  catch { memoryList.innerHTML = '<p class="memory-empty">暂时无法读取云端记忆</p>'; }
}
function renderMemories(items) {
  $("#memoryCount").textContent = items.length;
  if (!items.length) { memoryList.innerHTML = '<p class="memory-empty">还没有留下云端记忆</p>'; return; }
  memoryList.replaceChildren(...items.map((memory) => {
    const article = document.createElement("article"); article.className = "memory-item";
    if (memory.imageUrl) { const image = document.createElement("img"); image.className = "memory-thumb"; image.src = memory.imageUrl; image.alt = "当时的白板截图"; article.append(image); }
    const date = document.createElement("time"); date.className = "memory-date"; date.dateTime = memory.created_at; date.textContent = new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" }).format(new Date(memory.created_at));
    const written = document.createElement("p"); written.className = "memory-written"; written.textContent = memory.transcript || "（未辨认出文字）";
    const reply = document.createElement("p"); reply.className = "memory-reply"; reply.textContent = memory.reply;
    article.append(date, written, reply); return article;
  }));
}
function openMemories() { memoryPanel.classList.add("open"); memoryPanel.setAttribute("aria-hidden", "false"); $("#memoryButton").setAttribute("aria-expanded", "true"); scrim.hidden = false; }
function closeMemories() { memoryPanel.classList.remove("open"); memoryPanel.setAttribute("aria-hidden", "true"); $("#memoryButton").setAttribute("aria-expanded", "false"); scrim.hidden = true; }
