# Supabase 完整迁移设计

日期：2026-07-18

## 目标

把白板 Demo 从本地 Node 数据服务完整迁移到 Supabase。用户打开网页后自动匿名登录，提交白板截图即可获得智谱视觉模型回答；截图和记忆保存到用户自己的云端空间。现有本地测试数据不迁移，从空的云端数据开始。

## 已确认范围

- 使用 Supabase 匿名身份，无注册步骤。
- 使用 Supabase Edge Function 调用智谱 API。
- 智谱 API Key 只保存为 Edge Function Secret。
- 使用 Postgres 保存识别文字、回答和模型元数据。
- 使用私有 Storage Bucket 保存白板 PNG。
- 使用 RLS 和 Storage Policy 隔离不同用户的数据。
- 前端不再调用本地业务 API；本地 Node 服务仅用于静态开发预览。

## 架构

浏览器加载 Supabase 客户端后自动建立匿名会话。提交时，浏览器携带用户 JWT 调用 `ask-diary` Edge Function。函数校验身份、校验 PNG、读取用户近期记忆、调用智谱视觉模型、上传截图并写入记忆。函数成功后把新记忆返回前端。

记忆列表由浏览器通过 Supabase Data API 查询。截图位于私有 Bucket，前端按需创建短期签名 URL 显示缩略图。

```text
浏览器
  ├─ Supabase Auth：匿名身份
  ├─ Postgres：读取自己的 memories
  └─ ask-diary Edge Function
       ├─ 校验 JWT 与请求
       ├─ 读取近期 memories
       ├─ 调用智谱视觉模型
       ├─ 写入私有 Storage
       └─ 写入 Postgres
```

## 数据模型

`public.memories`：

- `id uuid primary key`
- `user_id uuid not null references auth.users(id)`
- `created_at timestamptz not null default now()`
- `transcript text not null`
- `reply text not null`
- `image_path text not null`
- `model text not null`

索引按 `(user_id, created_at desc)` 建立。RLS 允许已认证用户读取和删除自己的记录；新增记录由 Edge Function 以当前 JWT 执行，`user_id` 必须等于 `auth.uid()`。

Storage 使用私有 Bucket `diary-pages`，对象路径为 `<user_id>/<memory_id>.png`。策略仅允许路径首段等于当前用户 ID 的对象访问。

## Edge Function

`ask-diary` 接收 `{ imageDataUrl }`，只接受限定大小的 PNG Data URL。函数从 Supabase JWT 得到用户 ID，从环境 Secret 读取：

- `ZHIPU_API_KEY`
- `ZHIPU_API_BASE`（可选）
- `ZHIPU_MODEL`（可选）
- `ZHIPU_MODEL_FALLBACKS`（可选）

函数保留当前模型重试、模型降级和结构化响应解析逻辑。调用成功后才生成记忆 ID，上传 PNG，再插入数据库；若数据库写入失败，则尽力删除已上传对象，避免孤儿文件。

返回结构为 `{ memory }`。错误返回稳定的错误码和适合用户阅读的中文信息，不暴露供应商原始响应、Secret 或内部堆栈。

## 前端

前端增加 Supabase 初始化模块。配置只包含可以公开的 Project URL 与 Publishable/Anon Key，通过静态配置文件或构建环境注入。

页面启动流程：

1. 初始化 Supabase 客户端。
2. 恢复已有会话；没有会话时调用匿名登录。
3. 加载当前用户的记忆。
4. 用户提交 PNG 时调用 Edge Function。
5. 成功后展示回答并刷新记忆；失败时保留当前可恢复的交互状态并显示明确提示。

现有白板、鼠标、触屏、Apple Pencil 和响应式布局保持不变。

## 安全与限制

- 智谱 Key 不进入浏览器、Git、Postgres 或日志。
- 所有云端数据访问都要求有效 Supabase JWT。
- SQL 与 Storage 同时启用用户隔离策略。
- Edge Function 限制请求体、图片类型和图片大小。
- 匿名身份依赖浏览器本地会话；清除站点数据后会成为新用户，旧数据不会自动恢复。
- 初版不提供匿名账户合并、邮箱升级、管理员后台和旧数据导入。

## 测试与验收

- SQL 策略测试：用户只能读取自己的记忆和截图。
- Edge Function 单元测试：请求校验、模型降级、响应解析和错误映射。
- 前端测试：匿名登录、空记忆、提交、回答展示、记忆刷新和错误状态。
- 端到端测试：PC 与 iPad 尺寸各跑通至少一个“书写 → 提交 → 智谱回答 → 云端记忆 → 刷新后仍可读取”的真实案例。
- 安全检查：静态产物、Git 历史和浏览器网络请求中均不出现智谱 API Key。

## 部署输入

实施和真实验证需要：

- Supabase Project URL
- Supabase Publishable/Anon Key
- 已启用匿名登录的 Supabase 项目
- 通过 Supabase Secret 设置的 `ZHIPU_API_KEY`

这些值中只有 Project URL 和 Publishable/Anon Key允许出现在浏览器配置；智谱 Key 必须由 Secret 管理。
