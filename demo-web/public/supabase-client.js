import { createClient } from "https://esm.sh/@supabase/supabase-js@2.52.1";

let clientPromise;

export function getSupabase() {
  if (!clientPromise) clientPromise = initialize();
  return clientPromise;
}

async function initialize() {
  const config = window.RIDDLE_SUPABASE || {};
  if (!/^https:\/\/.+\.supabase\.co$/.test(config.url || "") || !config.publishableKey || config.publishableKey.startsWith("YOUR_")) {
    throw new Error("请先配置 Supabase Project URL 和 Publishable Key。");
  }
  const client = createClient(config.url, config.publishableKey, {
    auth: { persistSession: true, autoRefreshToken: true, detectSessionInUrl: false }
  });
  const { data: { session }, error: sessionError } = await client.auth.getSession();
  if (sessionError) throw sessionError;
  if (!session) {
    const { error } = await client.auth.signInAnonymously();
    if (error) throw new Error(`匿名登录失败：${error.message}`);
  }
  return client;
}

export async function askDiary(imageDataUrl) {
  const client = await getSupabase();
  const { data, error } = await client.functions.invoke("ask-diary", { body: { imageDataUrl } });
  if (error) {
    let message = error.message;
    try {
      const payload = await error.context?.json();
      if (payload?.error) message = payload.error;
    } catch { /* keep SDK error */ }
    throw new Error(message || "云端请求失败。");
  }
  return data.memory;
}

export async function listMemories() {
  const client = await getSupabase();
  const { data, error } = await client.from("memories")
    .select("id,created_at,transcript,reply,image_path,model")
    .order("created_at", { ascending: false }).limit(100);
  if (error) throw error;
  return Promise.all((data || []).map(async (memory) => {
    const { data: signed } = await client.storage.from("diary-pages").createSignedUrl(memory.image_path, 300);
    return { ...memory, imageUrl: signed?.signedUrl || "" };
  }));
}

