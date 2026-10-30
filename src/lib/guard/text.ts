import { t } from "../i18n/t";
import type { GuardFinding } from "../ipc";

/** What a flagged statement would do, in one sentence. */
export function consequence(f: GuardFinding): string {
  const object = f.object ?? t("guard.consequence.unknownObject");
  return t(`guard.consequence.${f.kind}`, { object });
}

/** The 1-based editor line of a finding whose 0-based line is relative to the text that was sent. */
export function findingLine(f: GuardFinding, lineOffset: number): number {
  return f.line + lineOffset + 1;
}
