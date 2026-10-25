import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { api } from "../api/client";
import type { AppError, ConnState, TopologyEvent, TopologyView } from "../api/types";
import { useConnectionEpochs } from "../state/connectionEpoch";

export const EVENTS_CAP = 500;

export const disconnectedView: TopologyView = {
  state: "disconnected",
  topology: null,
  catalogMember: null,
  catalogFallback: false,
  error: null,
};

type StatePayload = { connectionId: string; state: ConnState; error: AppError | null };
type TopologyPayload = { connectionId: string; view: TopologyView; newEvents: TopologyEvent[] };

/**
 * Запросы подключения, которые читают PostgreSQL: после отключения удаляются, после переподключения перечитываются.
 * Новый такой ключ добавляется сюда — и отключение, и переподключение (дерево, обзор) его учтут.
 */
export const PG_QUERY_KEYS = ["catalog", "memberFacts", "diag"] as const;

/** Предикат запросов подключения `id`, у которых первый элемент ключа — один из `prefixes`. */
export const ofConnection = (prefixes: readonly string[], id: string) => (q: { queryKey: readonly unknown[] }) =>
  prefixes.includes(q.queryKey[0] as string) && q.queryKey[1] === id;

/**
 * Подключение открыто или переоткрыто по действию пользователя (подключить, переподключить): запросы к PostgreSQL,
 * прочитанные раньше (каталог, факты узлов, вкладки объектов), перечитываются. Одно место для дерева и обзора,
 * чтобы пути подключения не расходились: отключение запросы удаляет, а ошибка чтения при закрытом подключении
 * иначе осталась бы в кэше до ручного повтора.
 */
export function refreshAfterConnect(qc: QueryClient, id: string) {
  void qc.invalidateQueries({ predicate: ofConnection(PG_QUERY_KEYS, id) });
  // Подписки вкладок на сэмплер перезапускаются: переподключение живого подключения не шлёт `stopped` и не
  // размонтирует вкладку, а остановка сэмплера после отказа во входе иначе длилась бы до переоткрытия вкладки.
  useConnectionEpochs.getState().bump(id);
}

const topologyKey = (id: string) => ["topology", id];
const eventsKey = (id: string) => ["topologyEvents", id];

/**
 * Событие новее любого ответа, запрошенного до него, поэтому выполняющуюся загрузку ключа отменяем
 * ДО записи. Иначе её запоздавший ответ перезаписал бы событие, и при `staleTime: Infinity` это
 * осталось бы навсегда: после отключения опрос остановлен и новых событий не будет.
 *
 * Отмена идёт с откатом (`revert: true` — второй аргумент, `CancelOptions`; задан явно, хотя это и
 * умолчание): `cancel` синхронно возвращает запрос в состояние до начала загрузки, а сама отменённая
 * загрузка в кэш уже ничего не пишет. Поэтому `setQueryData` сразу после вызова остаётся последним
 * значением. `await` здесь не нужен — он бы ещё и переставил события местами.
 */
function supersede(qc: QueryClient, key: readonly unknown[]) {
  void qc.cancelQueries({ queryKey: key, exact: true }, { revert: true });
}

/** Каноническая запись события: порядок ключей не зависит от источника (ответ команды или событие). */
// Массив-replacer у JSON.stringify не сортирует, а фильтрует ключи на всех уровнях (вложенное содержимое пропало бы
// и разные события склеились бы): сортируются пары верхнего уровня, значения сериализуются целиком.
const signature = (e: TopologyEvent) => JSON.stringify(Object.entries(e).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
/** Сколько верхних записей кэша сверяются с новыми событиями (ответ `list_topology_events` мог опередить событие). */
const DEDUPE_WINDOW = 50;

/**
 * В кэше нет данных, а событие описывает только часть картины (смена состояния, добавка к журналу):
 * синтезировать запись нельзя — при `staleTime: Infinity` она считалась бы свежей и полную картину
 * (например, историю, которую Rust хранит, пока вебвью перезагружался) никто бы не загрузил. Загрузку,
 * начатую до события, вызывающий уже отменил (`supersede`); здесь запрос помечается устаревшим:
 * активный наблюдатель сразу перечитает, следующий — прочитает при монтировании.
 */
function refetchFromSource(qc: QueryClient, key: readonly unknown[]) {
  void qc.invalidateQueries({ queryKey: key, exact: true });
}

export function applyState(qc: QueryClient, p: StatePayload) {
  const id = p.connectionId;
  if (p.state === "disconnected") {
    // Rust при отключении удаляет запись подключения целиком: вид — «отключено», журнал пуст. Событие —
    // полная картина, поэтому пишем его и без кэша.
    supersede(qc, topologyKey(id));
    supersede(qc, eventsKey(id));
    qc.setQueryData(topologyKey(id), disconnectedView);
    qc.setQueryData(eventsKey(id), []);
    qc.removeQueries({ predicate: ofConnection(PG_QUERY_KEYS, id) });
    return;
  }
  supersede(qc, topologyKey(id));
  const old = qc.getQueryData<TopologyView>(topologyKey(id));
  if (!old) {
    // Топология и каталог неизвестны (например, после перезагрузки вебвью) — читаем настоящий вид.
    refetchFromSource(qc, topologyKey(id));
    return;
  }
  qc.setQueryData<TopologyView>(topologyKey(id), { ...old, state: p.state, error: p.error });
}

export function applyTopology(qc: QueryClient, p: TopologyPayload) {
  const id = p.connectionId;
  supersede(qc, topologyKey(id));
  const old = qc.getQueryData<TopologyView>(topologyKey(id));
  // Вид в событии полный, его можно писать и без кэша.
  qc.setQueryData(topologyKey(id), p.view);
  if (old?.catalogMember && old.catalogMember !== p.view.catalogMember) {
    // Узел каталога сменился (например, после switchover) — каталог читается заново.
    void qc.invalidateQueries({ predicate: ofConnection(["catalog"], id) });
  }
  if (p.newEvents.length > 0) {
    supersede(qc, eventsKey(id));
    if (qc.getQueryData<TopologyEvent[]>(eventsKey(id)) === undefined) {
      // Есть только новые события, а история неизвестна — журнал читается целиком.
      refetchFromSource(qc, eventsKey(id));
    } else {
      // В событии — от старых к новым, в кэше — новые сверху.
      qc.setQueryData<TopologyEvent[]>(eventsKey(id), (prev = []) => {
        // Ответ list_topology_events, посчитанный в Rust после записи события, мог прийти раньше самого
        // события: такое событие в кэше уже есть и второй раз не добавляется.
        const seen = new Set(prev.slice(0, DEDUPE_WINDOW).map(signature));
        const fresh = p.newEvents.filter((e) => !seen.has(signature(e)));
        return [...[...fresh].reverse(), ...prev].slice(0, EVENTS_CAP);
      });
    }
  }
}

/** Подписка на события наблюдения — один раз, в Layout. */
export function useConnectionEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    // Подписка асинхронна: если хук размонтирован раньше, чем она готова, снимаем её сразу.
    // Вне Tauri (тесты, браузер) подписка отклоняется — это не ошибка.
    let unmounted = false;
    const offs: (() => void)[] = [];
    const subscribe = (p: Promise<() => void>) =>
      p.then((off) => (unmounted ? off() : offs.push(off))).catch(() => {});
    subscribe(listen<StatePayload>("connection://state", (e) => applyState(qc, e.payload)));
    subscribe(listen<TopologyPayload>("topology://changed", (e) => applyTopology(qc, e.payload)));
    return () => {
      unmounted = true;
      offs.forEach((off) => off());
    };
  }, [qc]);
}

export function useTopology(connectionId: string) {
  return useQuery({ queryKey: ["topology", connectionId], queryFn: () => api.getTopology(connectionId), staleTime: Infinity });
}

export function useTopologyEvents(connectionId: string) {
  return useQuery({
    queryKey: ["topologyEvents", connectionId],
    queryFn: () => api.listTopologyEvents(connectionId),
    staleTime: Infinity,
  });
}
