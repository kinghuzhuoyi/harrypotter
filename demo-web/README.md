# 浏览器白板 Demo

支持 PC 鼠标、触屏和 iPad Apple Pencil 的最小手写 AI 日记。

## 启动

1. 在[智谱开放平台](https://open.bigmodel.cn/)创建 API Key。
2. 复制 `.env.example` 为 `.env`，填写智谱 API Key。默认配置如下：

   ```env
   RIDDLE_OPENAI_KEY=your-zhipu-api-key
   RIDDLE_OPENAI_BASE=https://open.bigmodel.cn/api/paas/v4
   RIDDLE_OPENAI_MODEL=glm-4.6v-flash
   ```

   环境变量沿用原名称，但服务商已经切换为智谱。
3. 使用 Node.js 20 或更高版本运行 `npm.cmd start`。
4. PC 打开 `http://localhost:4173`；iPad 与电脑连接同一局域网后，打开启动日志中的 LAN 地址。

对话记录保存在 `data/memories.json`，白板截图保存在 `data/images/`。这些数据和 `.env` 均已忽略，不会提交到 Git。

Demo 默认监听所有局域网地址且没有鉴权，只应在可信网络运行。截图和近期文字记忆会发送给配置的模型服务商。
