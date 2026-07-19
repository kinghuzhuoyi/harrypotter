import { askDiary, getSupabase, listMemories } from "./supabase-client.js";
import {
  ABSORB_MS,
  ANSWER_FADE_MS,
  ANSWER_HOLD_MS,
  IDLE_SUBMIT_MS,
  characterDelay,
  isDismissSwipe
} from "./ink-timing.js";

const $ = (selector) => document.querySelector(selector);
const canvas = $("#paperCanvas");
const ctx = canvas.getContext("2d", { alpha: false });
const hint = $("#paperHint");
const clear = $("#clearButton");
const status = $("#statusText");
const answerCard = $("#answerCard");
const answer = $("#answerText");
const transcript = $("#transcriptText");
const sending = $("#sendingCard");
const memoryPanel = $("#memoryPanel");
const memoryList = $("#memoryList");
const scrim = $("#scrim");
const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

let state = "ready";
let drawing = false;
let hasInk = false;
let cloudReady = false;
let pointer = null;
let lastPoint = null;
let lastMidpoint = null;
let cssWidth = 0;
let cssHeight = 0;
let idleTimer = null;
let answerTimer = null;
let flowToken = 0;
let swipeStart = null;
let swipeEnd = null;

new ResizeObserver(resizeCanvas).observe(canvas);
canvas.addEventListener("pointerdown", startStroke);
canvas.addEventListener("pointermove", moveStroke);
canvas.addEventListener("pointerup", endStroke);
canvas.addEventListener("pointercancel", endStroke);
canvas.addEventListener("contextmenu", (event) => event.preventDefault());
clear.addEventListener("click", clearWriting);
answerCard.addEventListener("pointerdown", startDismissGesture);
answerCard.addEventListener("pointermove", moveDismissGesture);
answerCard.addEventListener("pointerup", endDismissGesture);
answerCard.addEventListener("pointercancel", endDismissGesture);
$("#memoryButton").addEventListener("click", openMemories);
$("#closeMemoryButton").addEventListener("click", closeMemories);
scrim.addEventListener("click", closeMemories);
addEventListener("keydown", (event) => { if (event.key === "Escape") closeMemories(); });
document.addEventListener("visibilitychange", () => {
  if (document.hidden) cancelIdleSubmit();
  else if (hasInk && !drawing && state === "idle-countdown") scheduleIdleSubmit();
});
initializeCloud();

function resizeCanvas() {
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const snapshot = hasInk ? canvas.toDataURL() : null;
  const ratio = Math.min(devicePixelRatio || 1, 2);
  cssWidth = rect.width;
  cssHeight = rect.height;
  canvas.width = Math.round(rect.width * ratio);
  canvas.height = Math.round(rect.height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  prepareInkContext();
  eraseCanvasPixels();
  if (snapshot) {
    const image = new Image();
    image.onload = () => ctx.drawImage(image, 0, 0, cssWidth, cssHeight);
    image.src = snapshot;
  }
}

function prepareInkContext() {
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.strokeStyle = "rgba(31, 27, 22, .92)";
  ctx.fillStyle = "rgba(31, 27, 22, .92)";
}

function startStroke(event) {
  if (!cloudReady || !["ready", "writing", "idle-countdown"].includes(state) || event.button > 0) return;
  event.preventDefault();
  cancelIdleSubmit();
  state = "writing";
  status.textContent = "纸上正留下墨迹";
  pointer = event.pointerId;
  canvas.setPointerCapture(pointer);
  drawing = true;
  lastPoint = penPoint(event, null);
  lastMidpoint = null;
  drawDot(lastPoint, 0.62);
  markInk();
}

function moveStroke(event) {
  if (!drawing || event.pointerId !== pointer) return;
  event.preventDefault();
  for (const item of event.getCoalescedEvents?.() || [event]) {
    const next = penPoint(item, lastPoint);
    drawSmoothSegment(lastPoint, next);
    lastPoint = next;
  }
  markInk();
}

function endStroke(event) {
  if (event.pointerId !== pointer) return;
  if (lastPoint && lastMidpoint) drawTaperedEnd(lastMidpoint, lastPoint);
  drawing = false;
  pointer = null;
  lastPoint = null;
  lastMidpoint = null;
  if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
  state = "idle-countdown";
  scheduleIdleSubmit();
}

function penPoint(event, previous) {
  const rect = canvas.getBoundingClientRect();
  const x = event.clientX - rect.left;
  const y = event.clientY - rect.top;
  const time = event.timeStamp || performance.now();
  const pressure = event.pointerType === "mouse" ? 0.38 : Math.max(0.12, event.pressure || 0.38);
  const distance = previous ? Math.hypot(x - previous.x, y - previous.y) : 0;
  const elapsed = previous ? Math.max(1, time - previous.time) : 16;
  const speed = distance / elapsed;
  const slowWeight = Math.max(0, 1.25 - Math.min(1.25, speed * 1.8));
  return { x, y, time, width: 1.15 + pressure * 4.9 + slowWeight };
}

function drawDot(point, scale = 1) {
  ctx.beginPath();
  ctx.arc(point.x, point.y, point.width * scale / 2, 0, Math.PI * 2);
  ctx.fill();
}

function drawSmoothSegment(from, to) {
  const midpoint = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 };
  ctx.beginPath();
  ctx.moveTo(lastMidpoint?.x ?? from.x, lastMidpoint?.y ?? from.y);
  ctx.quadraticCurveTo(from.x, from.y, midpoint.x, midpoint.y);
  ctx.lineWidth = (from.width + to.width) / 2;
  ctx.globalAlpha = 0.86 + Math.min(0.1, to.width / 70);
  ctx.stroke();
  ctx.globalAlpha = 1;
  lastMidpoint = midpoint;
}

function drawTaperedEnd(from, to) {
  ctx.beginPath();
  ctx.moveTo(from.x, from.y);
  ctx.quadraticCurveTo(to.x, to.y, to.x, to.y);
  ctx.lineWidth = Math.max(1, to.width * 0.52);
  ctx.globalAlpha = 0.82;
  ctx.stroke();
  ctx.globalAlpha = 1;
}

function markInk() {
  hasInk = true;
  hint.hidden = true;
  clear.disabled = false;
}

function scheduleIdleSubmit() {
  cancelIdleSubmit();
  status.textContent = "纸张正在聆听";
  idleTimer = setTimeout(submitAutomatically, IDLE_SUBMIT_MS);
}

function cancelIdleSubmit() {
  if (idleTimer) clearTimeout(idleTimer);
  idleTimer = null;
}

function clearWriting() {
  if (!["ready", "writing", "idle-countdown"].includes(state)) return;
  cancelIdleSubmit();
  flowToken += 1;
  eraseCanvasPixels();
  hasInk = false;
  drawing = false;
  state = "ready";
  hint.hidden = false;
  clear.disabled = true;
  status.textContent = "准备书写";
}

async function submitAutomatically() {
  cancelIdleSubmit();
  if (!hasInk || drawing || !cloudReady || state !== "idle-countdown") return;
  const token = ++flowToken;
  const imageDataUrl = exportPng();
  state = "absorbing";
  clear.disabled = true;
  status.textContent = "墨迹正被纸张吸收";
  const request = askDiary(imageDataUrl).then((value) => ({ value }), (error) => ({ error }));
  await absorbInk(token);
  if (token !== flowToken) return;
  state = "waiting";
  sending.hidden = false;
  status.textContent = "纸张正在思考";
  const result = await request;
  if (token !== flowToken) return;
  sending.hidden = true;
  const memory = result.error
    ? { transcript: "", reply: `纸张没有听清：${result.error.message}` }
    : result.value;
  await writeAnswer(memory, token);
  if (!result.error) loadMemories();
}

async function absorbInk(token) {
  canvas.classList.add("absorbing");
  if (!reduceMotion) await wait(ABSORB_MS);
  if (token !== flowToken) return;
  eraseCanvasPixels();
  hasInk = false;
  canvas.classList.remove("absorbing");
}

function exportPng() {
  const output = document.createElement("canvas");
  const scale = Math.min(1, 1280 / canvas.width);
  output.width = Math.round(canvas.width * scale);
  output.height = Math.round(canvas.height * scale);
  const out = output.getContext("2d", { alpha: false });
  out.fillStyle = "#fbf8ef";
  out.fillRect(0, 0, output.width, output.height);
  out.drawImage(canvas, 0, 0, output.width, output.height);
  return output.toDataURL("image/png");
}

function eraseCanvasPixels() {
  ctx.globalAlpha = 1;
  ctx.fillStyle = "#fbf8ef";
  ctx.fillRect(0, 0, cssWidth, cssHeight);
  prepareInkContext();
}

async function writeAnswer(memory, token) {
  answer.textContent = "";
  transcript.textContent = memory.transcript || "未能可靠辨认";
  answerCard.hidden = false;
  answerCard.classList.remove("fading");
  state = "writing-answer";
  status.textContent = "纸张正在回应";
  const characters = Array.from(memory.reply || "墨水没有留下回答。");
  if (reduceMotion) answer.textContent = characters.join("");
  else {
    for (const character of characters) {
      if (token !== flowToken) return;
      answer.textContent += character;
      await wait(characterDelay(character));
    }
  }
  if (token !== flowToken) return;
  state = "reading";
  status.textContent = "纸上的回答会停留片刻";
  answerTimer = setTimeout(() => fadeAnswer(token), ANSWER_HOLD_MS);
}

async function fadeAnswer(token = flowToken) {
  if (!['reading', 'writing-answer'].includes(state) || token !== flowToken) return;
  if (answerTimer) clearTimeout(answerTimer);
  answerTimer = null;
  state = "fading-answer";
  answerCard.classList.add("fading");
  if (!reduceMotion) await wait(ANSWER_FADE_MS);
  if (token !== flowToken) return;
  answerCard.hidden = true;
  answerCard.classList.remove("fading");
  answer.textContent = "";
  transcript.textContent = "";
  state = "ready";
  hint.hidden = false;
  status.textContent = "准备书写";
}

function startDismissGesture(event) {
  if (state !== "reading") return;
  event.preventDefault();
  answerCard.setPointerCapture(event.pointerId);
  swipeStart = { x: event.clientX, y: event.clientY };
  swipeEnd = swipeStart;
}
function moveDismissGesture(event) {
  if (!swipeStart || state !== "reading") return;
  event.preventDefault();
  swipeEnd = { x: event.clientX, y: event.clientY };
}
function endDismissGesture(event) {
  if (!swipeStart || state !== "reading") return;
  event.preventDefault();
  swipeEnd = { x: event.clientX, y: event.clientY };
  const shouldDismiss = isDismissSwipe(swipeStart, swipeEnd);
  if (answerCard.hasPointerCapture(event.pointerId)) answerCard.releasePointerCapture(event.pointerId);
  swipeStart = null;
  swipeEnd = null;
  if (shouldDismiss) fadeAnswer();
}

async function initializeCloud() {
  clear.disabled = true;
  status.textContent = "正在连接云端";
  try {
    await getSupabase();
    cloudReady = true;
    status.textContent = "准备书写";
    await loadMemories();
  } catch (error) {
    status.textContent = error.message;
    memoryList.innerHTML = '<p class="memory-empty">云端尚未配置</p>';
  }
}

async function loadMemories() {
  try { renderMemories(await listMemories()); }
  catch { memoryList.innerHTML = '<p class="memory-empty">暂时无法读取云端记忆</p>'; }
}

function renderMemories(items) {
  $("#memoryCount").textContent = items.length;
  if (!items.length) {
    memoryList.innerHTML = '<p class="memory-empty">还没有留下云端记忆</p>';
    return;
  }
  memoryList.replaceChildren(...items.map((memory) => {
    const article = document.createElement("article");
    article.className = "memory-item";
    if (memory.imageUrl) {
      const image = document.createElement("img");
      image.className = "memory-thumb";
      image.src = memory.imageUrl;
      image.alt = "当时的白板截图";
      article.append(image);
    }
    const date = document.createElement("time");
    date.className = "memory-date";
    date.dateTime = memory.created_at;
    date.textContent = new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" }).format(new Date(memory.created_at));
    const written = document.createElement("p");
    written.className = "memory-written";
    written.textContent = memory.transcript || "（未辨认出文字）";
    const reply = document.createElement("p");
    reply.className = "memory-reply";
    reply.textContent = memory.reply;
    article.append(date, written, reply);
    return article;
  }));
}

function openMemories() {
  memoryPanel.classList.add("open");
  memoryPanel.setAttribute("aria-hidden", "false");
  $("#memoryButton").setAttribute("aria-expanded", "true");
  scrim.hidden = false;
}
function closeMemories() {
  memoryPanel.classList.remove("open");
  memoryPanel.setAttribute("aria-hidden", "true");
  $("#memoryButton").setAttribute("aria-expanded", "false");
  scrim.hidden = true;
}

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
