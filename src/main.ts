import { createApp } from "vue";
import { createPinia } from "pinia";
// 内置 Cascadia Mono 等宽字体（自托管，随应用打包，避免跨平台字库缺失）
import "@fontsource/cascadia-mono/400.css";
import "@fontsource/cascadia-mono/700.css";
import "./styles.css";

/** 根据窗口类型按需加载根组件，避免不同窗口互相加载完整模块图 */
async function bootstrap(): Promise<void> {
  const search = new URLSearchParams(window.location.search);
  const rootComponent =
    search.get("window") === "editor"
      ? (await import("./components/TextEditorWindow.vue")).default
      : (await import("./App.vue")).default;
  const app = createApp(rootComponent);
  app.use(createPinia());
  app.mount("#app");
}

void bootstrap();
