import test from "node:test";
import assert from "node:assert/strict";
import { isModelBusy, parseModelTurn, postWithModelFallback, userFacingApiError } from "../oracle-client.js";

test("classifies Zhipu 1305 as model congestion instead of quota exhaustion", () => {
  const error = { code: "1305", message: "该模型当前访问量过大，请您稍后再试" };
  assert.equal(isModelBusy(429, error), true);
  assert.match(userFacingApiError(429, error), /模型当前繁忙/);
  assert.doesNotMatch(userFacingApiError(429, error), /额度不足/);
});

test("retries a busy model then falls back to the next free model", async () => {
  const calls = [];
  const replies = [
    response(429, { error: { code: "1305" } }),
    response(429, { error: { code: "1305" } }),
    response(200, { choices: [{ message: { content: "ok" } }] })
  ];
  const result = await postWithModelFallback({
    base: "https://example.test/v4",
    key: "test-key",
    models: ["primary", "fallback"],
    body: { messages: [] },
    fetchImpl: async (_url, options) => {
      calls.push(JSON.parse(options.body).model);
      return replies.shift();
    },
    wait: async () => {}
  });
  assert.deepEqual(calls, ["primary", "primary", "fallback"]);
  assert.equal(result.response.ok, true);
  assert.equal(result.model, "fallback");
});

test("extracts JSON from Zhipu thinking and answer wrappers", () => {
  const raw = '<think>private reasoning with {"reply":"wrong"}</think><answer>{"transcript":"HI","reply":"Hello."}</answer>';
  assert.deepEqual(parseModelTurn(raw), { transcript: "HI", reply: "Hello." });
});

test("extracts JSON from markdown code fences", () => {
  assert.deepEqual(parseModelTurn('```json\n{"transcript":"你好","reply":"你好。"}\n```'), {
    transcript: "你好",
    reply: "你好。"
  });
});

function response(status, payload) {
  return { ok: status >= 200 && status < 300, status, json: async () => payload };
}
