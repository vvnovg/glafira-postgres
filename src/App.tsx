import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Layout } from "./components/Layout";
import { useAppMenu } from "./lib/useAppMenu";
import { useLanguageSync } from "./lib/useLanguageSync";

function LanguageSync() {
  useLanguageSync();
  return null;
}

/** Меню Help на macOS (на других системах ничего не делает). */
function AppMenu() {
  useAppMenu();
  return null;
}

// Без автоповтора: backend сам ждёт до таймаута и быстро отказывает при недоступном кластере,
// а повтор удваивал бы ожидание. Повторить можно кнопкой в баннере ошибки.
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <LanguageSync />
      <AppMenu />
      <Layout />
    </QueryClientProvider>
  );
}
