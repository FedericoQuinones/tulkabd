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
  if (!Number.isInteger(form.port) || form.port < 1 || form.port > 65535) {
    errors.port = "connection.errors.portRange";
  }
  return errors;
}
