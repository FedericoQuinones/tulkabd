import type { ConnectionConfig, Environment } from "../ipc";

/** The TLS and environment values of a connection form. */
export interface TlsForm {
  environment: Environment;
  encrypt: boolean;
  trustServerCertificate: boolean;
}

/**
 * Whether saving `form` without a new password would remove the password saved for `stored` because
 * of weaker TLS (encryption turned off or certificate trust turned on). Mirrors the TLS part of
 * `may_reuse_saved_password` in tulka-app: it does not matter when both the stored and the edited
 * environment are Local.
 */
export function weakerTlsDropsPassword(
  stored: Pick<ConnectionConfig, "environment" | "encrypt" | "trustServerCertificate" | "passwordSaved"> | undefined,
  form: TlsForm,
  typedPassword: string,
): boolean {
  if (!stored?.passwordSaved || typedPassword !== "") return false;
  if (stored.environment === "local" && form.environment === "local") return false;
  return (!form.encrypt && stored.encrypt) || (form.trustServerCertificate && !stored.trustServerCertificate);
}
