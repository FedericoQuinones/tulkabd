import type { Environment, GuardLevel } from "../ipc";

/** Defaults per environment; mirrors `Environment::default_color/default_guard` in tulka-core. */
export const ENVIRONMENTS: {
  id: Environment;
  label: string;
  token: string;
  color: string;
  guard: GuardLevel;
}[] = [
  { id: "production", label: "env.production", token: "--env-prod", color: "#e5566f", guard: "confirm" },
  { id: "staging", label: "env.staging", token: "--env-stage", color: "#d9a441", guard: "off" },
  { id: "test", label: "env.test", token: "--env-test", color: "#44a282", guard: "off" },
  { id: "local", label: "env.local", token: "--env-local", color: "#4a8ad6", guard: "off" },
];

export interface EnvState {
  environment: Environment;
  color: string;
  guard: GuardLevel;
  colorTouched: boolean;
  guardTouched: boolean;
}

/** Changes the environment; color and guard follow its defaults unless edited by hand. */
export function applyEnvironment(state: EnvState, environment: Environment): EnvState {
  const def = ENVIRONMENTS.find((e) => e.id === environment)!;
  return {
    ...state,
    environment,
    color: state.colorTouched ? state.color : def.color,
    guard: state.guardTouched ? state.guard : def.guard,
  };
}

/** Initial state for the dialog: touched flags are set only when the stored values differ from the environment defaults. */
export function initialEnvState(stored?: { environment: Environment; color: string; guard: GuardLevel }): EnvState {
  const environment = stored?.environment ?? "local";
  const def = ENVIRONMENTS.find((e) => e.id === environment)!;
  const color = stored?.color ?? def.color;
  const guard = stored?.guard ?? def.guard;
  return { environment, color, guard, colorTouched: color !== def.color, guardTouched: guard !== def.guard };
}
