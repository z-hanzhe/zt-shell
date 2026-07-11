import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
// 内置 Cascadia Mono 等宽字体（自托管，随应用打包，避免跨平台字库缺失）
import "@fontsource/cascadia-mono/400.css";
import "@fontsource/cascadia-mono/700.css";
import "./styles.css";

const app = createApp(App);
app.use(createPinia());
app.mount("#app");
