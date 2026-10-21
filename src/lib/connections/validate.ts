export interface ConnectionForm {
  name: string;
  server: string;
  port: number;
}

/** Maps each invalid field to the i18n key of its error. An empty map means valid. */
export function validateConnection(form: ConnectionForm): Record<string, string> {
  const errors: Record<string, string> = {};
  if (form.name.trim() === "") errors.name = "connection.errors.nameRequired";
  if (form.server.trim() === "") errors.server = "connection.errors.serverRequired";
  else if (!validServer(form.server)) errors.server = "connection.errors.serverInstance";
  if (!Number.isInteger(form.port) || form.port < 1 || form.port > 65535) {
    errors.port = "connection.errors.portRange";
  }
  return errors;
}

/** `host` or `host\instance`, with both parts non-empty. */
function validServer(server: string): boolean {
  const i = server.indexOf("\\");
  if (i < 0) return true;
  const host = server.slice(0, i).trim();
  const instance = server.slice(i + 1).trim();
  return host !== "" && instance !== "" && !instance.includes("\\");
}

/** Whether `server` names an instance (`host\instance`). */
export function isNamedInstance(server: string): boolean {
  return server.includes("\\");
}
