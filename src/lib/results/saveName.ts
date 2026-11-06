import type { SaveFormat } from "../ipc";

const pad = (n: number) => String(n).padStart(2, "0");

/** The suggested name of a saved result: `result-20261106-153042.csv`, in local time. */
export function defaultSaveName(format: SaveFormat, now = new Date()): string {
  const date = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
  const time = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `result-${date}-${time}.${format}`;
}
