import type { Environment } from "../api/types";

/** Цвета окружений — токены палитры (спец. U1 §2): в тёмной теме берутся её значения. */
export const ENV_COLORS: Record<Environment, string> = {
  local: "var(--muted)",
  dev: "var(--accent)",
  test: "var(--ok)",
  stage: "var(--warn)",
  prod: "var(--badge-danger)",
};

export const ENV_LABELS: Record<Environment, string> = {
  local: "LOCAL",
  dev: "DEV",
  test: "TEST",
  stage: "STAGE",
  prod: "PROD",
};
