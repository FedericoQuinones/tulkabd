<script lang="ts">
  import { t } from "../i18n/t";
</script>

<footer class="status">
  <span>{t("status.noConnection")}</span>
  <span class="r">{t("status.ready")}</span>
</footer>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 0 14px;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 12px;
  }
  .r {
    margin-left: auto;
    font-family: var(--mono);
  }
</style>
