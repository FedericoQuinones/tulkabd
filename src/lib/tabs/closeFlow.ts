/** What a tab's session holds that closing it could lose. */
export type CloseCheck = { tabId: string; state: "none" | "open" | "unknown"; count: number };

/** What closing these tabs needs from the user. */
export function closePlan(checks: CloseCheck[]): { ask: false } | { ask: true; tabs: CloseCheck[] } {
  const tabs = checks.filter((c) => c.state !== "none");
  return tabs.length > 0 ? { ask: true, tabs } : { ask: false };
}

/** The text of a command rejection (a string, or an object holding one in `value`). */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "value" in e) return String((e as { value: unknown }).value);
  return String(e);
}

/**
 * Turns the answer of `transactionCount` into a check. A lost connection has nothing left to
 * lose (the server rolled back), and a tab without a session counts 0. A tab that is running
 * ("busy") or any other failure is "unknown": it may hold a transaction, so the user is asked.
 */
export function checkFrom(tabId: string, result: { count: number } | { error: unknown }): CloseCheck {
  if ("count" in result) {
    return result.count > 0 ? { tabId, state: "open", count: result.count } : { tabId, state: "none", count: 0 };
  }
  if (errorText(result.error) === "disconnected") return { tabId, state: "none", count: 0 };
  return { tabId, state: "unknown", count: 0 };
}
