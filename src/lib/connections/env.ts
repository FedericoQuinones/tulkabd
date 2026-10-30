import type { AiAccess, Environment, EnvironmentStyle, GuardLevel } from "../ipc";

/** What a new connection of each environment starts with: the saved preferences' `environments`. */
export type EnvironmentDefaults = Record<Environment, EnvironmentStyle>;

/** Labels and tokens per environment, with the factory color and guard (as `Environment::default_color/default_guard` in tulka-core). */
export const ENVIRONMENTS: {
  id: Environment;
  label: string;
  token: string;
  color: string;
  guard: GuardLevel;
  aiAccess: AiAccess;
}[] = [
  { id: "production", label: "env.production", token: "--env-prod", color: "#e5566f", guard: "confirm", aiAccess: "none" },
  { id: "staging", label: "env.staging", token: "--env-stage", color: "#d9a441", guard: "off", aiAccess: "readOnly" },
  { id: "test", label: "env.test", token: "--env-test", color: "#44a282", guard: "off", aiAccess: "readOnly" },
  { id: "local", label: "env.local", token: "--env-local", color: "#4a8ad6", guard: "off", aiAccess: "readOnly" },
];

export interface EnvState {
  environment: Environment;
  color: string;
  guard: GuardLevel;
  aiAccess: AiAccess;
  colorTouched: boolean;
  guardTouched: boolean;
  aiTouched: boolean;
}

/** Changes the environment; color, guard and AI access follow its defaults unless edited by hand. */
export function applyEnvironment(state: EnvState, environment: Environment, defaults?: EnvironmentDefaults): EnvState {
  const def = defaults?.[environment] ?? ENVIRONMENTS.find((e) => e.id === environment)!;
  return {
    ...state,
    environment,
    color: state.colorTouched ? state.color : def.color,
    guard: state.guardTouched ? state.guard : def.guard,
    aiAccess: state.aiTouched ? state.aiAccess : def.aiAccess,
  };
}

/** Initial state for the dialog: touched flags are set only when the stored values differ from the environment defaults (the preferences', when given). */
export function initialEnvState(
  stored?: { environment: Environment; color: string; guard: GuardLevel; aiAccess: AiAccess },
  defaults?: EnvironmentDefaults,
): EnvState {
  const environment = stored?.environment ?? "local";
  const def = defaults?.[environment] ?? ENVIRONMENTS.find((e) => e.id === environment)!;
  const color = stored?.color ?? def.color;
  const guard = stored?.guard ?? def.guard;
  const aiAccess = stored?.aiAccess ?? def.aiAccess;
  return {
    environment,
    color,
    guard,
    aiAccess,
    colorTouched: color !== def.color,
    guardTouched: guard !== def.guard,
    aiTouched: aiAccess !== def.aiAccess,
  };
}
