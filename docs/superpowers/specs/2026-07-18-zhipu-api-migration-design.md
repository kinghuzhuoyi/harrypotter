# 智谱 API 迁移设计

## 目标

将浏览器白板 Demo 的视觉模型服务从 OpenAI 默认配置切换为智谱开放平台，同时保持白板、截图、记忆和回复数据结构不变。

## 方案

继续使用智谱提供的 OpenAI 兼容接口，不引入 SDK。服务端仍向 `/chat/completions` 发送由文本和 PNG data URL 组成的多模态消息。

- Base URL：`https://open.bigmodel.cn/api/paas/v4`
- 默认模型：`glm-4.6v-flash`
- 鉴权：继续使用 `Authorization: Bearer <API Key>`
- 环境变量：保留现有 `RIDDLE_OPENAI_*` 名称，避免无关迁移

## 变更范围

- 更新 `.env.example`、本地 `.env` 和服务端默认值。
- 更新 README 中的配置说明。
- 将常见服务商错误转换成简短中文信息，避免原始错误撑满回复页面。
- 不改变前端请求、记忆文件和历史图片格式。

## 验证

- 服务端语法检查通过。
- 重启后 `/api/config` 返回智谱默认模型。
- 对智谱 Base URL 完成网络连通性检查。
- 未配置或无效 Key 时返回简短、可操作的中文错误。

