# 浏览器白板 Demo

支持 PC 鼠标、触屏和 iPad Apple Pencil。业务后端已完整迁移到 Supabase：匿名身份由 Supabase Auth 管理，记忆写入 Postgres，截图写入私有 Storage，智谱请求由 Edge Function 发起。

## Supabase 项目配置

1. 创建 Supabase 项目，在 Dashboard 的 Authentication 设置中启用 Anonymous Sign-Ins。
2. 安装 Supabase CLI，在仓库根目录登录并连接项目：

   ```sh
   supabase login
   supabase link --project-ref YOUR_PROJECT_REF
   ```

3. 部署数据库 migration、智谱 Secret 和 Edge Function：

   ```sh
   supabase db push
   supabase secrets set ZHIPU_API_KEY=YOUR_ZHIPU_API_KEY
   supabase functions deploy ask-diary
   ```

   可选 Secret：`ZHIPU_API_BASE`、`ZHIPU_MODEL`、`ZHIPU_MODEL_FALLBACKS`。不要把真实智谱 Key 写进仓库。

4. 从 Supabase Dashboard 的 API 设置复制 Project URL 和 Publishable Key（旧项目也可以使用 Anon Key），填写 `public/supabase-config.js`：

   ```js
   window.RIDDLE_SUPABASE = {
     url: "https://YOUR_PROJECT_REF.supabase.co",
     publishableKey: "sb_publishable_..."
   };
   ```

Project URL 和 Publishable/Anon Key 本来就是浏览器端公开配置；数据安全由用户 JWT、RLS 和 Storage Policy 保证。智谱 Key 只能存在于 Supabase Edge Function Secret。

## 本地启动

使用 Node.js 20 或更高版本：

```sh
cd demo-web
npm start
```

PC 打开 `http://localhost:4173`。iPad 与电脑连接同一局域网后，打开启动日志中的 LAN 地址。本地 Node 服务现在只提供静态文件，不再保存数据或持有智谱 Key。

## 云端资源

- Migration：`supabase/migrations/20260718000100_create_memories.sql`
- Edge Function：`supabase/functions/ask-diary/index.ts`
- 私有 Bucket：`diary-pages`
- 数据表：`public.memories`

匿名用户清除浏览器站点数据后会生成新的身份，无法再读取旧匿名身份的数据。
