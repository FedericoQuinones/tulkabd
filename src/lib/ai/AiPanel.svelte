<script lang="ts">
  import { t } from "../i18n/t";
  import AiChat from "./AiChat.svelte";
  import AiConnect from "./AiConnect.svelte";
</script>

<section class="ai" aria-label={t("ai.panel.label")}>
  <AiConnect />
  <div class="chat" data-ai-chat><AiChat /></div>
</section>

<style>
  .ai {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .chat {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
