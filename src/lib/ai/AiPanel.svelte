<script lang="ts">
  import { t } from "../i18n/t";
  import AiConnect from "./AiConnect.svelte";
</script>

<div class="ai" aria-label={t("ai.panel.label")}>
  <AiConnect />
  <!-- The chat is mounted here, below the connect section. -->
  <div class="chat" data-ai-chat></div>
</div>

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
