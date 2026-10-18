import type { Environment } from "../api/types";

export const ENV_COLORS: Record<Environment, string> = {
  local: "#8a8f98",
  dev: "#3b82f6",
  test: "#16a34a",
  stage: "#f59e0b",
  prod: "#dc2626",
};

export const ENV_LABELS: Record<Environment, string> = {
  local: "LOCAL",
  dev: "DEV",
  test: "TEST",
  stage: "STAGE",
  prod: "PROD",
};
