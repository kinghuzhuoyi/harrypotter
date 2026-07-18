export async function postWithModelFallback({ base, key, models, body, fetchImpl = fetch, wait = sleep }) {
  let last = null;
  for (const model of unique(models)) {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      if (attempt > 0) await wait(800);
      const response = await fetchImpl(`${base}/chat/completions`, {
        method: "POST",
        headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, model }),
        signal: AbortSignal.timeout(45_000)
      });
      const payload = await response.json().catch(() => ({}));
      last = { response, payload, model };
      if (response.ok) return last;
      if (!isModelBusy(response.status, payload.error)) return last;
    }
  }
  return last;
}

export function isModelBusy(status, providerError) {
  return status === 429 && String(providerError?.code || "") === "1305";
}

export function userFacingApiError(status, providerError) {
  const message = String(providerError?.message || "");
  const code = String(providerError?.code || "");
  if (isModelBusy(status, providerError)) return "智谱视觉模型当前繁忙，已尝试备用模型，请稍后重试。";
  if (status === 401 || /invalid.*(key|token)|令牌|鉴权/i.test(message)) return "智谱 API Key 无效，请检查 demo-web/.env。";
  if (status === 429 || /quota|余额|次数|rate.?limit/i.test(`${code} ${message}`)) return "智谱 API 额度不足或请求过快，请检查账户余额后重试。";
  if (status === 400 && /model/i.test(message)) return "当前智谱模型名称或请求格式不受支持，请检查模型配置。";
  if (status >= 500) return "智谱服务暂时不可用，请稍后重试。";
  return message ? `智谱请求失败：${message.slice(0, 160)}` : `智谱接口返回 HTTP ${status}。`;
}

export function parseModelTurn(raw) {
  const text = Array.isArray(raw) ? raw.map((part) => part.text || "").join("") : String(raw);
  const withoutThinking = text.replace(/<think>[\s\S]*?<\/think>/gi, "").trim();
  const answer = withoutThinking.match(/<answer>([\s\S]*?)<\/answer>/i)?.[1] || withoutThinking;
  const candidate = answer.replace(/^```(?:json)?\s*/i, "").replace(/\s*```$/, "").trim();
  const jsonCandidate = candidate.match(/\{[\s\S]*\}/)?.[0] || candidate;
  try {
    const parsed = JSON.parse(jsonCandidate);
    return {
      transcript: String(parsed.transcript || "").trim(),
      reply: String(parsed.reply || "").trim() || "墨水没有留下回答。"
    };
  } catch {
    try {
      const repaired = JSON.parse(
        jsonCandidate.replace(/\\"/g, '"').replace(/\\n/g, "\n").replace(/\\r/g, "\r").replace(/\\t/g, "\t")
      );
      return {
        transcript: String(repaired.transcript || "").trim(),
        reply: String(repaired.reply || "").trim() || "墨水没有留下回答。"
      };
    } catch {
      return { transcript: "", reply: candidate || "墨水没有留下回答。" };
    }
  }
}

function unique(values) {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
