import en from "./en.json";

const messages: Record<string, string> = en;

/** Looks up a UI string and replaces `{name}` placeholders with `params.name`. */
export function t(key: string, params?: Record<string, string | number>): string {
  const message = messages[key];
  if (message === undefined) {
    if (import.meta.env.DEV) console.warn(`[i18n] missing key: ${key}`);
    return key;
  }
  if (!params) return message;
  return message.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}
