<script setup lang="ts">
/** 安全展示更新说明中的标题、列表和段落，所有内容均按文本渲染。 */
import { computed } from "vue";

const props = defineProps<{ content: string }>();
type Block = { kind: "heading" | "paragraph" | "list" | "ordered"; lines: string[] };
const blocks = computed(() => {
  const result: Block[] = [];
  let previous: Block | undefined;
  for (const line of props.content.replace(/\r\n/g, "\n").split("\n")) {
    if (!line.trim()) { previous = undefined; continue; }
    const heading = line.match(/^#{1,6}\s+(.+)$/);
    const bullet = line.match(/^\s*[-*+]\s+(.+)$/);
    const numbered = line.match(/^\s*\d+[.)]\s+(.+)$/);
    const kind = heading ? "heading" : bullet ? "list" : numbered ? "ordered" : "paragraph";
    const text = (heading ?? bullet ?? numbered)?.[1] ?? line;
    if (previous?.kind === kind && kind !== "heading") previous.lines.push(text);
    else { previous = { kind, lines: [text] }; result.push(previous); }
  }
  return result;
});
</script>

<template>
  <div class="release-notes">
    <template v-for="(block, index) in blocks" :key="index">
      <h3 v-if="block.kind === 'heading'">{{ block.lines[0] }}</h3>
      <ul v-else-if="block.kind === 'list'"><li v-for="(line, i) in block.lines" :key="i">{{ line }}</li></ul>
      <ol v-else-if="block.kind === 'ordered'"><li v-for="(line, i) in block.lines" :key="i">{{ line }}</li></ol>
      <p v-else>{{ block.lines.join('\n') }}</p>
    </template>
  </div>
</template>

<style scoped>
.release-notes { line-height: 1.9; user-select: text; overflow-wrap: anywhere; }
h3 { margin: 16px 0 8px; font-size: 13px; font-weight: 600; }
p { white-space: pre-wrap; margin: 8px 0; }
ul, ol { padding-left: 20px; margin: 8px 0; }
li + li { margin-top: 4px; }
.release-notes > :first-child { margin-top: 0; }
.release-notes > :last-child { margin-bottom: 0; }
</style>
