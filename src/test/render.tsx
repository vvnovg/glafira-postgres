import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";

export function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrap = (el: ReactElement) => <QueryClientProvider client={client}>{el}</QueryClientProvider>;
  const result = render(wrap(ui));
  // rerender обёрнут в тот же провайдер и тот же клиент: кэш запросов между отрисовками сохраняется.
  return { ...result, rerender: (el: ReactElement) => result.rerender(wrap(el)), client };
}
