import { createClient } from "https://esm.sh/@supabase/supabase-js@2.52.1";

const corsHeaders = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
};
const MAX_IMAGE_BYTES = 6 * 1024 * 1024;

Deno.serve(async (request) => {
  if (request.method === "OPTIONS") return new Response("ok", { headers: corsHeaders });
  if (request.method !== "POST") return json(405, { error: "仅支持 POST 请求。" });

  try {
    const authorization = request.headers.get("Authorization");
    if (!authorization) return json(401, { error: "登录状态已失效，请刷新页面。" });

    const supabaseUrl = requiredEnv("SUPABASE_URL");
    const anonKey = requiredEnv("SUPABASE_ANON_KEY");
    const client = createClient(supabaseUrl, anonKey, {
      global: { headers: { Authorization: authorization } },
      auth: { persistSession: false },
    });
    const { data: { user }, error: authError } = await client.auth.getUser();
    if (authError || !user) return json(401, { error: "登录状态已失效，请刷新页面。" });

    const body = await request.json().catch(() => ({}));
    const image = decodePng(body.imageDataUrl);
    if (!image) return json(400, { error: "缺少有效的白板 PNG。" });
    if (image.length > MAX_IMAGE_BYTES) return json(413, { error: "白板截图过大，请清空后重新书写。" });

    const { data: history, error: historyError } = await client
      .from("memories")
      .select("transcript,reply,created_at")
      .order("created_at", { ascending: false })
      .limit(6);
    if (historyError) throw new PublicError("暂时无法读取云端记忆。", 502);

    const turn = await askZhipu(body.imageDataUrl, [...(history || [])].reverse());
    const id = crypto.randomUUID();
    const imagePath = `${user.id}/${id}.png`;
    const { error: uploadError } = await client.storage
      .from("diary-pages")
      .upload(imagePath, image, { contentType: "image/png", upsert: false });
    if (uploadError) throw new PublicError("回答已生成，但截图暂时无法保存，请重试。", 502);

    const memory = {
      id,
      user_id: user.id,
      transcript: turn.transcript,
      reply: turn.reply,
      image_path: imagePath,
      model: turn.model,
    };
    const { data: saved, error: insertError } = await client
      .from("memories")
      .insert(memory)
      .select("id,created_at,transcript,reply,image_path,model")
      .single();
    if (insertError) {
      await client.storage.from("diary-pages").remove([imagePath]);
      throw new PublicError("回答已生成，但云端记忆暂时无法保存，请重试。", 502);
    }
    return json(200, { memory: saved });
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    if (error instanceof PublicError) return json(error.status, { error: error.message });
    return json(500, { error: "纸张暂时没有回应，请稍后重试。" });
  }
});

async function askZhipu(imageDataUrl: string, history: Array<Record<string, string>>) {
  const base = (Deno.env.get("ZHIPU_API_BASE") || "https://open.bigmodel.cn/api/paas/v4").replace(/\/$/, "");
  const primary = Deno.env.get("ZHIPU_MODEL") || "glm-4.6v-flash";
  const fallbacks = (Deno.env.get("ZHIPU_MODEL_FALLBACKS") || "glm-4.1v-thinking-flash,glm-4v-flash").split(",");
  const models = [...new Set([primary, ...fallbacks].map((item) => item.trim()).filter(Boolean))];
  const system = [
    "You are the quiet spirit of a handwritten diary.",
    "Read the handwriting in the supplied whiteboard image and answer in the same language.",
    "Be warm, concise and slightly mysterious. Reply in one to three sentences.",
    "Return ONLY valid JSON with string fields transcript and reply.",
    "Never mention images, OCR, models or AI. If unreadable, say the ink is unclear.",
  ].join(" ");
  const recent = history.map(({ transcript, reply, created_at }) => ({ written: transcript, answer: reply, date: created_at }));
  const userText = recent.length
    ? `Recent diary memories, oldest first:\n${JSON.stringify(recent)}\n\nRead and answer the new writing.`
    : "Read and answer the new writing.";
  let lastStatus = 500;
  let lastError: Record<string, unknown> = {};

  for (const model of models) {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      if (attempt) await new Promise((resolve) => setTimeout(resolve, 800));
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 45_000);
      const response = await fetch(`${base}/chat/completions`, {
        method: "POST",
        headers: { Authorization: `Bearer ${requiredEnv("ZHIPU_API_KEY")}`, "Content-Type": "application/json" },
        body: JSON.stringify({
          model,
          temperature: 0.7,
          max_tokens: 500,
          messages: [
            { role: "system", content: system },
            { role: "user", content: [{ type: "text", text: userText }, { type: "image_url", image_url: { url: imageDataUrl } }] },
          ],
        }),
        signal: controller.signal,
      }).finally(() => clearTimeout(timeout));
      const payload = await response.json().catch(() => ({}));
      if (response.ok) {
        const raw = payload.choices?.[0]?.message?.content;
        if (!raw) throw new PublicError("模型没有返回回答。", 502);
        return { ...parseModelTurn(raw), model };
      }
      lastStatus = response.status;
      lastError = payload.error || {};
      if (!(response.status === 429 && String(payload.error?.code || "") === "1305")) break;
    }
  }
  throw new PublicError(userFacingApiError(lastStatus, lastError), 502);
}

function parseModelTurn(raw: unknown) {
  const text = Array.isArray(raw) ? raw.map((part) => part?.text || "").join("") : String(raw);
  const withoutThinking = text.replace(/<think>[\s\S]*?<\/think>/gi, "").trim();
  const candidate = (withoutThinking.match(/<answer>([\s\S]*?)<\/answer>/i)?.[1] || withoutThinking)
    .replace(/^```(?:json)?\s*/i, "").replace(/\s*```$/, "").trim();
  const jsonCandidate = candidate.match(/\{[\s\S]*\}/)?.[0] || candidate;
  for (const value of [jsonCandidate, jsonCandidate.replace(/\\"/g, '"').replace(/\\n/g, "\n")]) {
    try {
      const parsed = JSON.parse(value);
      return { transcript: String(parsed.transcript || "").trim(), reply: String(parsed.reply || "").trim() || "墨水没有留下回答。" };
    } catch { /* try repaired form */ }
  }
  return { transcript: "", reply: candidate || "墨水没有留下回答。" };
}

function decodePng(value: unknown) {
  if (typeof value !== "string" || !value.startsWith("data:image/png;base64,")) return null;
  try {
    const binary = atob(value.slice(value.indexOf(",") + 1));
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    return bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47 ? bytes : null;
  } catch { return null; }
}

function userFacingApiError(status: number, providerError: Record<string, unknown>) {
  const message = String(providerError?.message || "");
  const code = String(providerError?.code || "");
  if (status === 429 && code === "1305") return "智谱视觉模型当前繁忙，已尝试备用模型，请稍后重试。";
  if (status === 401 || /invalid.*(key|token)|令牌|鉴权/i.test(message)) return "智谱 API Key 无效，请检查 Supabase Secret。";
  if (status === 429 || /quota|余额|次数|rate.?limit/i.test(`${code} ${message}`)) return "智谱 API 额度不足或请求过快，请检查账户余额后重试。";
  if (status >= 500) return "智谱服务暂时不可用，请稍后重试。";
  return "智谱请求失败，请稍后重试。";
}

function requiredEnv(name: string) {
  const value = Deno.env.get(name);
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
}

function json(status: number, value: unknown) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { ...corsHeaders, "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store" },
  });
}

class PublicError extends Error {
  constructor(message: string, readonly status: number) { super(message); }
}
