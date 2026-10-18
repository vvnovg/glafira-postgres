# Глоссарий переводов Glafira

Единые решения по терминам и стилю для всех словарей `src/locales/*.json`.
Эталон ключей и параметров — `en.json`, эталон смысла — `ru.json`.
Меняя решение здесь, исправьте его во всех словарях одним коммитом.

## Общие правила
- Коротко и нейтрально, как в системных приложениях. Без восклицаний и шуток.
- Параметры `{{…}}` и теги `<code>…</code>`, `<offset/>`, `<total/>`, `<file>`, `<name>` переносятся без изменений,
  их нельзя переводить, склонять или удалять. Порядок внутри фразы можно менять.
- Значение параметра (имя топика, подключения, группы) не склоняется: ставьте перед ним
  существительное в нужном падеже и берите имя в кавычки языка: pl `topik „{{name}}”`, de `Topic „{{name}}“`.
- Многоточие — один символ `…`, не `...`.
- Тире в паузе — `—` с пробелами (во французском тоже), в диапазонах — `–`.
- Единицы: `ms`, `KB`, `MB` как в en; число и единица разделяются неразрывным пробелом там, где он уже есть в en.
- Плюралы: заполняются все формы языка (см. таблицу в README), даже если формы совпадают по тексту.

## Не переводится
Kafka, Glafira, Pro, Free, SASL, SSL, TLS, mTLS, PEM, PKCS#12, JKS, JCEKS, CSV, JSON, JSON Lines, librdkafka,
DNS, TCP, ID, MCP, LLM, stdio, Claude Desktop, Claude Code, Cursor, Build (сборка: подпись в «О программе» и слово в тексте об обновлениях — «build», не «версия»/«компиляция»; в ja — ビルド, как в остальном словаре), Leader (лидер партиции; в ja и ko — リーダー и 리더); имена параметров Kafka (`cleanup.policy`, `retention.ms`, `acks`, `enable.idempotence`); значения acks (0, 1, all), стратегии earliest/latest, пресеты delete/compact/test;
команды, пути, адреса (`docker compose -f dev/docker-compose.yml up -d`, `keytool …`, `localhost:9092`);
название экрана «Producer» (как в ru); значения CreateTime / LogAppendTime, producer / uncompressed в compression.type.

## Стиль по языкам

| Язык | Обращение | Кнопки и пункты меню | Вопрос-подтверждение | Кавычки | Особенности |
|---|---|---|---|---|---|
| de | Sie, по возможности без обращения | инфинитив: Speichern, Löschen | «Topic „{{name}}“ löschen?» | „…“ | составные слова через дефис с англ. терминами: Topic-Name, Consumer-Group-ID |
| fr | vous, по возможности без обращения | инфинитив: Enregistrer, Supprimer | «Supprimer le topic « {{name}} » ?» | « … » с U+00A0 внутри | U+00A0 перед `:` `;` `!` `?` и после `«` / перед `»` |
| es | tú, по возможности без обращения | инфинитив: Guardar, Eliminar | «¿Eliminar el topic “{{name}}”?» | “…” | `¿…?`, `¡…!` |
| it | tu, по возможности без обращения | повелительное как в macOS/Windows: Salva, Elimina | «Eliminare il topic “{{name}}”?» | “…” | артикль перед англ. терминами: il topic, l’offset |
| pl | безличные формы, без «Ty/Pan» | повелительное: Zapisz, Usuń | «Usunąć topik „{{name}}”?» | „…” | значения параметров только в именительном после существительного |
| pt-BR | você, по возможности без обращения | инфинитив: Salvar, Excluir | «Excluir o tópico “{{name}}”?» | “…” | бразильская норма: arquivo, tela, excluir (не ficheiro, ecrã, eliminar) |
| zh-CN | 你, по возможности без обращения | глагол: 保存、删除 | «删除主题“{{name}}”？» | “…” | полноширинная пунктуация; пробел между иероглифами и латиницей/цифрами |
| zh-TW | 你, по возможности без обращения | глагол: 儲存、刪除 | «要刪除主題「{{name}}」嗎？» | 「…」 | тайваньская лексика (檔案, 程式, 設定), не перекодированный zh-CN |
| ja | без местоимений; в тексте です/ます | краткие формы: 保存, 削除, 閉じる | «トピック「{{name}}」を削除しますか？» | 「…」 | без пробелов; заимствования катаканой по глоссарию |
| ko | без местоимений; в тексте -습니다/-ㅂ니다 | существительные: 저장, 삭제 | «'{{name}}' 토픽을 삭제하시겠습니까?» | '…' | пробелы между словами; ASCII-пунктуация; частица — после постоянного слова, не после параметра |

## Термины

| en | ru | de | fr | es | it | pl | pt-BR | zh-CN | zh-TW | ja | ko |
|---|---|---|---|---|---|---|---|---|---|---|---|
| topic | топик | Topic (das, Pl. Topics) | topic (m.) | topic (m.) | topic (m., pl. invar.) | topik (m.: topiku, topiki, topików) | tópico (m.) | 主题 | 主題 | トピック | 토픽 |
| partition | партиция | Partition (die) | partition (f.) | partición (f.) | partizione (f.) | partycja | partição (f.) | 分区 | 分區 | パーティション | 파티션 |
| offset | оффсет | Offset (der, Pl. Offsets) | offset (m.) | offset (m.) | offset (m., pl. invar.) | offset (m.: offsetu, offsety) | offset (m.) | 偏移量 | 偏移量 | オフセット | 오프셋 |
| consumer group | consumer group | Consumer Group (die) | consumer group (m.) | consumer group (m.) | consumer group (m.) | grupa konsumentów | consumer group (m.) | 消费者组 | 消費者群組 | コンシューマーグループ | 컨슈머 그룹 |
| broker | брокер | Broker (der) | broker (m.) | broker (m.) | broker (m.) | broker (m.: brokera) | broker (m.) | Broker | Broker | ブローカー | 브로커 |
| producer / consumer (роль) | producer / consumer | Producer / Consumer | producteur / consommateur | productor / consumidor | producer / consumer | producent / konsument | produtor / consumidor | 生产者 / 消费者 | 生產者 / 消費者 | プロデューサー / コンシューマー | 프로듀서 / 컨슈머 |
| lag | лаг | Lag (der) | lag (m.) | lag (m.) | lag (m.) | lag (m.: lagu) | lag (m.) | 延迟 (в тексте можно 消费延迟) | 延遲 (в тексте можно 消費延遲) | ラグ | 랙 |
| message | сообщение | Nachricht | message (m.) | mensaje | messaggio | wiadomość | mensagem | 消息 | 訊息 | メッセージ | 메시지 |
| key / value | ключ / значение | Schlüssel / Wert | clé / valeur | clave / valor | chiave / valore | klucz / wartość | chave / valor | 键 / 值 | 鍵 / 值 | キー / 値 | 키 / 값 |
| headers | заголовки | Header (Pl.) | en-têtes | encabezados | intestazioni | nagłówki | cabeçalhos | 标头 | 標頭 | ヘッダー | 헤더 |
| connection | подключение | Verbindung | connexion | conexión | connessione | połączenie | conexão | 连接 | 連線 | 接続 | 연결 |
| folder | папка | Ordner | dossier | carpeta | cartella | folder | pasta | 文件夹 | 資料夾 | フォルダー | 폴더 |
| prod connection | prod-подключение | Prod-Verbindung | connexion prod | conexión prod | connessione prod | połączenie prod | conexão prod | 生产环境连接 | 正式環境連線 | 本番接続 | 운영 환경 연결 |
| cluster | кластер | Cluster | cluster | clúster | cluster | klaster | cluster | 集群 | 叢集 | クラスター | 클러스터 |
| bootstrap server | bootstrap-сервер | Bootstrap-Server | serveur bootstrap | servidor bootstrap | server bootstrap | serwer bootstrap | servidor bootstrap | Bootstrap 服务器 | Bootstrap 伺服器 | ブートストラップサーバー | 부트스트랩 서버 |
| consumer session | consumer-сессия | Consumer-Session (die, Pl. Sessions) | session consumer | sesión de consumer | sessione consumer | sesja konsumenta | sessão de consumer | 消费者会话 | 消費者工作階段 | コンシューマーセッション | 컨슈머 세션 |
| read-only | только чтение | Schreibgeschützt | Lecture seule | Solo lectura | Sola lettura | Tylko do odczytu | Somente leitura | 只读 | 唯讀 | 読み取り専用 | 읽기 전용 |
| preset (набор настроек при создании топика) | пресет | Voreinstellung | préréglage | preajuste | profilo | profil (не «szablon» — так названы шаблоны продюсера) | predefinição | 预设 | 預設集 (не «預設» — это «по умолчанию») | プリセット | 프리셋 |
| committed offset | закоммиченный оффсет | committeter Offset | offset commité | offset con commit | offset sottoposto a commit | zatwierdzony offset | offset com commit | 已提交的偏移量 | 已提交的偏移量 | コミット済みオフセット | 커밋된 오프셋 |
| reset offsets | сбросить оффсеты | Offsets zurücksetzen | Réinitialiser les offsets | Restablecer offsets | Reimposta gli offset | Zresetuj offsety | Redefinir offsets | 重置偏移量 | 重設偏移量 | オフセットをリセット | 오프셋 재설정 |
| retention | хранение (retention) | Aufbewahrung | rétention | retención | conservazione | retencja | retenção | 保留 | 保留 | 保持 | 보존 |
| replication factor | фактор репликации | Replikationsfaktor | facteur de réplication | factor de replicación | fattore di replica | współczynnik replikacji | fator de replicação | 副本因子 | 複寫係數 | レプリケーションファクター | 복제 계수 |
| under-replicated | недореплицированные | unterrepliziert | sous-répliquées | subreplicadas | sotto-replicate | niedoreplikowane | sub-replicadas | 副本不足 | 複本不足 | レプリカ不足 | 복제 부족 |
| trusted CA | доверенный CA | vertrauenswürdige CA | CA de confiance | CA de confianza | CA attendibile | zaufany CA | CA confiável | 受信任的 CA | 受信任的 CA | 信頼されたCA | 신뢰할 수 있는 CA |
| client certificate / key | сертификат / ключ клиента | Client-Zertifikat / -Schlüssel | certificat / clé client | certificado / clave de cliente | certificato / chiave client | certyfikat / klucz klienta | certificado / chave do cliente | 客户端证书 / 私钥 | 用戶端憑證 / 私密金鑰 | クライアント証明書 / クライアント秘密鍵 | 클라이언트 인증서 / 개인 키 |
| system secret store | системное хранилище секретов | Schlüsselbund des Systems | trousseau du système | almacén de secretos del sistema | archivio dei segreti di sistema | systemowy magazyn sekretów | cofre de segredos do sistema | 系统密钥存储 | 系統金鑰儲存區 | システムの資格情報ストア | 시스템 보안 저장소 |
| idempotence | идемпотентность | Idempotenz | idempotence | idempotencia | idempotenza | idempotencja | idempotência | 幂等性 | 冪等性 | 冪等性 | 멱등성 |
| compression | сжатие | Komprimierung | compression | compresión | compressione | kompresja | compressão | 压缩 | 壓縮 | 圧縮 | 압축 |
| rebalance | ребалансировка | Rebalance (das) | rééquilibrage | rebalanceo | ribilanciamento | rebalans | rebalanceamento | 重平衡 | 重新平衡 | リバランス | 리밸런싱 |
| placeholder (Producer) | плейсхолдер | Platzhalter | variable | marcador | segnaposto | symbol zastępczy | marcador | 占位符 | 預留位置 | プレースホルダー | 플레이스홀더 |
| hint (значок «?») | подсказка | Hinweis | aide | ayuda | suggerimento | podpowiedź | dica | 提示 | 提示 | ヒント | 도움말 |
| segment (лога) | сегмент | Segment | segment | segmento | segmento | segment | segmento | 日志段 | 日誌區段 | セグメント | 세그먼트 |
| compaction | compaction | Compaction (die) | compaction (f.) | compactación | compattazione | kompaktowanie | compactação | Compaction (не «压缩» — это compression) | Compaction (не «壓縮» — это compression) | コンパクション | 컴팩션 |
| tiered storage | tiered storage | Tiered Storage | tiered storage | tiered storage | tiered storage | tiered storage | tiered storage | 分层存储 | 分層儲存 | 階層型ストレージ | 계층형 스토리지 |
| follower (реплика) | фолловер | Follower | follower | follower | follower | follower | follower | Follower | Follower | フォロワー | 팔로워 |
| license | лицензия | Lizenz | licence | licencia | licenza | licencja | licença | 许可证 | 授權 | ライセンス | 라이선스 |
| settings | настройки | Einstellungen | Paramètres | Ajustes | Impostazioni | Ustawienia | Configurações | 设置 | 設定 | 設定 | 설정 |
| about | о программе | Über Glafira | À propos | Acerca de | Informazioni | O programie | Sobre | 关于 Glafira | 關於 Glafira | Glafiraについて | Glafira 정보 |
| update | обновление | Update | mise à jour | actualización | aggiornamento | aktualizacja | atualização | 更新 | 更新 | アップデート | 업데이트 |
| diagnostics archive | архив диагностики | Diagnosearchiv | archive de diagnostic | archivo de diagnóstico | archivio diagnostico | archiwum diagnostyczne | arquivo de diagnóstico | 诊断归档 | 診斷壓縮檔 | 診断アーカイブ | 진단 아카이브 |
| set by your organization | задан вашей организацией | von Ihrer Organisation festgelegt | défini par votre organisation | establecido por tu organización | impostato dalla tua organizzazione | ustawione przez organizację | definido pela sua organização | 由你的组织设置 | 由你的組織設定 | 言語は組織によって設定されています | 언어는 조직에서 설정합니다 |
| System default (…) | Как в системе (…) | Systemsprache ({{name}}) | Langue du système ({{name}}) | Idioma del sistema ({{name}}) | Lingua di sistema ({{name}}) | Język systemu ({{name}}) | Idioma do sistema ({{name}}) | 跟随系统（{{name}}） | 跟隨系統（{{name}}） | システムの言語（{{name}}） | 시스템 언어({{name}}) |
| LLM assistant | LLM-ассистент | LLM-Assistent | assistant LLM | asistente LLM | assistente LLM | asystent LLM | assistente LLM | LLM 助手 | LLM 助理 | LLMアシスタント | LLM 어시스턴트 |
| MCP server | MCP-сервер | MCP-Server | serveur MCP | servidor MCP | server MCP | serwer MCP | servidor MCP | MCP 服务器 | MCP 伺服器 | MCPサーバー | MCP 서버 |
| client (MCP) | клиент | Client (der, Pl. Clients) | client (m.) | cliente (m.) | client (m., pl. invar.) | klient | cliente (m.) | 客户端 | 用戶端 | クライアント | 클라이언트 |
| approve / reject (a client) | разрешить / отклонить | zulassen / ablehnen | autoriser / rejeter | permitir / rechazar | consenti / rifiuta | zezwól / odrzuć | permitir / rejeitar | 允许 / 拒绝 | 允許 / 拒絕 | 許可 / 拒否 | 허용 / 거부 |
| approved (client) | подтверждённый | genehmigt | approuvé | aprobado | approvato | zatwierdzony | aprovado | 已批准 | 已核准 | 承認済み | 승인된 |
| revoke | отозвать | widerrufen | révoquer | revocar | revoca | odwołaj | revogar | 撤销 | 撤銷 | 取り消す | 승인 취소 (кнопка; в журнале — 접근 취소됨) |
| coordinator (group) | координатор | Koordinator | coordinateur | coordinador | coordinatore | koordynator | coordenador | 协调器 | 協調器 | コーディネーター | 코디네이터 |

## Частые слова интерфейса

| en | de | fr | es | it | pl | pt-BR | zh-CN | zh-TW | ja | ko |
|---|---|---|---|---|---|---|---|---|---|---|
| Save | Speichern | Enregistrer | Guardar | Salva | Zapisz | Salvar | 保存 | 儲存 | 保存 | 저장 |
| Cancel | Abbrechen | Annuler | Cancelar | Annulla | Anuluj | Cancelar | 取消 | 取消 | キャンセル | 취소 |
| Delete | Löschen | Supprimer | Eliminar | Elimina | Usuń | Excluir | 删除 | 刪除 | 削除 | 삭제 |
| Close | Schließen | Fermer | Cerrar | Chiudi | Zamknij | Fechar | 关闭 | 關閉 | 閉じる | 닫기 |
| Retry | Erneut versuchen | Réessayer | Reintentar | Riprova | Spróbuj ponownie | Tentar novamente | 重试 | 重試 | 再試行 | 다시 시도 |
| Loading… | Wird geladen… | Chargement… | Cargando… | Caricamento… | Wczytywanie… | Carregando… | 加载中… | 載入中… | 読み込み中… | 불러오는 중… |
| Details | Details | Détails | Detalles | Dettagli | Szczegóły | Detalhes | 详细信息 | 詳細資訊 | 詳細 | 세부 정보 |
| Apply | Anwenden | Appliquer | Aplicar | Applica | Zastosuj | Aplicar | 应用 | 套用 | 適用 | 적용 |
| Back | Zurück | Retour | Atrás | Indietro | Wstecz | Voltar | 返回 | 返回 | 戻る | 뒤로 |
| Search | Suchen | Rechercher | Buscar | Cerca | Szukaj | Pesquisar | 搜索 | 搜尋 | 検索 | 검색 |
| Refresh | Aktualisieren | Actualiser | Actualizar | Aggiorna | Odśwież | Atualizar | 刷新 | 重新整理 | 更新 | 새로 고침 |
| Create | Erstellen | Créer | Crear | Crea | Utwórz | Criar | 创建 | 建立 | 作成 | 만들기 |
| Edit | Bearbeiten | Modifier | Editar | Modifica | Edytuj | Editar | 编辑 | 編輯 | 編集 | 편집 |
| Send | Senden | Envoyer | Enviar | Invia | Wyślij | Enviar | 发送 | 傳送 | 送信 | 보내기 |
| Copy | Kopieren | Copier | Copiar | Copia | Kopiuj | Copiar | 复制 | 複製 | コピー | 복사 |
| Export | Exportieren | Exporter | Exportar | Esporta | Eksportuj | Exportar | 导出 | 匯出 | エクスポート | 내보내기 |
| Import | Importieren | Importer | Importar | Importa | Importuj | Importar | 导入 | 匯入 | インポート | 가져오기 |

## Правила письма: китайский, японский, корейский

Проверяет `pnpm i18n:check` (словари и главы руководства, вне `` `кода` ``, `{{…}}` и тегов): в zh-CN, zh-TW, ja нет
полуширинных `, . : ; ? !` сразу после иероглифа или каны; в zh-CN, zh-TW нет пробела между двумя иероглифами.
Остальное — ручная проверка по этому разделу и `review-checklist.md`.

### Общее для zh-CN, zh-TW, ja, ko
- Множественное число — одна форма `_other` (`Intl.PluralRules` этих языков); счётные слова — внутри фразы:
  zh-CN «{{count}} 条消息», «{{count}} 个主题»; zh-TW «{{count}} 則訊息», «{{count}} 個主題»; ja «{{count}} 件のメッセージ»;
  ko «메시지 {{count}}개».
- Названия Kafka, протоколов, форматов и параметров — латиницей, как в «Не переводится»; экран «Producer» — латиницей.
  Broker в zh — латиницей (Broker), в ja и ko — устоявшиеся ブローカー и 브로커; Leader и Follower — латиницей в zh, リーダー/フォロワー в ja, 리더/팔로워 в ko.
  Lag — в интерфейсе и подсказках коротко 延迟 / 延遲 (в прозе руководства можно 消费延迟 / 消費延遲), ラグ, 랙.
- Многоточие — один символ `…`, как во всех языках (не «……» и не `...`).
- Числа, единицы (`ms`, `MB`) и латиница — полуширинные; полноширинные цифры и латиницу (`１２`, `ＭＢ`) не использовать.
- Имя (параметр `{{name}}`) не склоняется и не меняет соседние слова: в кавычках языка и рядом с существительным
  («主题“{{name}}”», «トピック「{{name}}」», «'{{name}}' 토픽»).

### zh-CN и zh-TW
- Пунктуация полноширинная: «，。：；？！（）»; перечисление однородных слов — «、».
  Внутри латинского фрагмента (адрес, параметр, команда) — как в оригинале: `localhost:9092`, `acks=all`.
- Кавычки: zh-CN “…” (вложенные ‘…’), zh-TW 「…」 (вложенные 『…』).
- Пробел между иероглифом и латиницей, цифрой, `{{count}}`, `` `кодом` ``: «连接 Kafka 集群», «已加载 {{count}} 条消息»,
  «设置 `retention.ms`». Не ставится между иероглифом и полноширинной пунктуацией и между двумя иероглифами.
- Обращение «你», если без него не обойтись (как в системах Apple и Microsoft); «您» не использовать.
- zh-TW — тайваньская норма, а не механическая замена знаков zh-CN:

| смысл | zh-CN | zh-TW |
|---|---|---|
| файл | 文件 | 檔案 |
| программа / приложение | 程序 / 应用 | 程式 / 應用程式 |
| настройки | 设置 | 設定 |
| по умолчанию | 默认 | 預設 |
| сервер | 服务器 | 伺服器 |
| сеть | 网络 | 網路 |
| данные | 数据 | 資料 |
| клиент | 客户端 | 用戶端 |
| сертификат / ключ шифрования | 证书 / 密钥 | 憑證 / 金鑰 |
| загрузить (данные) | 加载 | 載入 |
| запустить | 运行 | 執行 |
| окно | 窗口 | 視窗 |
| поддержка | 支持 | 支援 |
| информация | 信息 | 資訊 |
| сессия | 会话 | 工作階段 |
| журнал (логи) | 日志 | 日誌 |
| Java keystore / truststore | 密钥库 / 信任库 | 金鑰庫 / 信任儲存區 (не «金鑰儲存區» — это системное хранилище секретов, 系統金鑰儲存區) |

### ja
- В предложениях — です/ます; на кнопках и в пунктах меню — краткие формы (保存, 削除, 閉じる, 再試行).
- Заимствования — катаканой по этому глоссарию; долгий гласный в конце слова пишется: サーバー, フォルダー, ユーザー,
  パラメーター, コンシューマー, プロデューサー.
- Пунктуация полноширинная: «、。：？！（）»; кавычки 「…」 (вложенные 『…』).
- Пробелов нет — ни между японскими словами, ни между японским текстом и латиницей или числом («Kafkaクラスター»,
  「{{count}}件のメッセージ」); пробел остаётся только внутри латинской фразы («Schema Registry»).

### ko
- В предложениях — официально-вежливый стиль (-습니다/-ㅂ니다); на кнопках — существительные или краткие формы (저장, 삭제, 닫기).
- Просьбы и указания — -하세요, не -십시오 («토픽을 지정하세요», «다시 시도하세요»); вопросы-подтверждения — -하시겠습니까?.
- Пробелы между словами по нормам 띄어쓰기; пунктуация ASCII: `, . : ? !` и `( )`; кавычки для имён — `'…'`.
- Частица не ставится сразу после `{{…}}`: её форма зависит от последнего слога значения. Ставьте после параметра
  постоянное слово («'{{name}}' 토픽을…», «메시지 {{count}}개를…»), а не «{{name}}을(를)».

### Руководство (docs/guide)
- В zh-CN, zh-TW, ja метка ставится вплотную к тексту: «打开[[ui:sidebar.settings]]。», «[[ui:sidebar.settings]]を開きます。»;
  исключение в zh-CN и zh-TW: если значение метки начинается или кончается латиницей или цифрой, с этой стороны ставится
  обычный пробел между иероглифом и латиницей («在 [[ui:connections.form.bootstrap]]中», «打开 [[ui:sidebar.producer]] 标签页»);
  в ko — с пробелами по 띄어쓰기 («[[ui:sidebar.settings]] 화면»).
- Если значение метки начинается или кончается пунктуацией («导出…», «（高级）», «「…」»), `**…**` вплотную к иероглифу
  по CommonMark не станет жирным (ни в приложении, ни на GitHub). `pnpm i18n:check` такое находит; перестройте фразу так,
  чтобы рядом с меткой была пунктуация, или поставьте метку в конец предложения.
- Абзац — одной строкой: перенос строки внутри абзаца между иероглифами в приложении отображается как пробел.

## Слова руководства (de)

Термины, которые часто встречаются в `docs/guide/de` и отсутствуют в словаре интерфейса:
tab — Tab (der, Pl. Tabs); screen — Bildschirm; panel (левая панель) — Bereich; button — Schaltfläche;
checkbox — Kontrollkästchen; logs — Protokolle (Protokolldateien); batch send — Stapelversand, Stapel;
release page — Release-Seite; installer — Installationsprogramm; support request — Supportanfrage;
macOS menu Help — Menü „Hilfe“.
Глава «LLM-Assistenten (MCP)»: gateway — Gateway; snippet — Fragment; local socket — lokaler Socket; named pipe — Named Pipe; token — Token; indicator — Anzeige; badge `LLM` — Zeichen.

## Слова руководства (fr)

Термины, которые часто встречаются в `docs/guide/fr` и отсутствуют в словаре интерфейса:
tab — onglet; screen — écran; panel (левая панель) — panneau; button — bouton; checkbox — case à cocher;
logs — journaux (fichiers journaux); batch send — envoi par lot, lot; batch (Kafka) — batch;
release page — page des versions; installer — programme d’installation; support request — demande d’assistance;
support — assistance; timestamp — horodatage; tombstone — tombstone; listener — listener;
macOS menu Help — menu « Aide »; to commit — commiter; read-only connection — connexion en lecture seule
(с меткой: «connexions en [[ui:common.readOnly]]»).
Глава «Assistants LLM (MCP)»: gateway — passerelle; snippet — fragment; local socket — socket local; named pipe — tube nommé; token — jeton; indicator — indicateur; badge `LLM` — « marquées `LLM` ».

## Слова руководства (es)

Термины, которые часто встречаются в `docs/guide/es` и отсутствуют в словаре интерфейса:
tab — pestaña; screen — pantalla; panel (левая панель) — panel; button — botón; checkbox — casilla;
logs — registros (archivos de registro, как «Registros:» в интерфейсе); batch send — envío por lotes, lote; batch (Kafka) — batch;
release page — página de versiones; installer — instalador; build — build (m.); support request — solicitud de soporte;
support — soporte; timestamp — marca de tiempo; tombstone — tombstone; listener — listener; handshake — handshake;
purge — purgar, purga (как в интерфейсе); to commit — hacer commit; committed offsets — offsets con commit;
macOS menu Help — menú Ayuda; Keychain — Llavero; Credential Manager — Administrador de credenciales;
read-only connection — conexión de solo lectura (с меткой: «en las conexiones con la opción [[ui:common.readOnly]] activada»).
Глава «Asistentes LLM (MCP)»: gateway — pasarela; snippet — fragmento; named pipe — canalización con nombre; token — token; indicator — indicador; badge `LLM` — marca.

## Слова руководства (it)

Термины, которые часто встречаются в `docs/guide/it` и отсутствуют в словаре интерфейса:
tab — scheda; screen — schermata; panel (левая панель) — pannello; button — pulsante; checkbox — casella di controllo;
form — modulo (как в UI: Svuota modulo); template — modello; history — cronologia; logs — log (m., file di log);
batch send — invio in batch (как в UI); batch (Kafka) — batch; release page — pagina delle release;
installer — programma di installazione; build — build (f.); support request — richiesta di assistenza; support — assistenza;
timestamp — timestamp; tombstone — tombstone; listener — listener; handshake — handshake; policy — policy (f., как в UI);
browse (просмотр сообщений) — consultazione, lettura; purge — svuotare, svuotamento (как в UI: Svuota);
to commit — eseguire il commit; committed offset — offset sottoposto a commit; lock icon — icona a forma di lucchetto;
macOS menu Help — menu Aiuto; Keychain — Portachiavi; Credential Manager — Gestione credenziali;
read-only connection — connessione in sola lettura (с меткой: «sulle connessioni con l’opzione [[ui:common.readOnly]] attiva»).
Глава «Assistenti LLM (MCP)»: gateway — gateway; snippet — frammento; named pipe — named pipe; token — token; indicator — indicatore; badge `LLM` — contrassegno.

## Слова руководства (pl)

Термины, которые часто встречаются в `docs/guide/pl` и отсутствуют в словаре интерфейса:
регистр — безличный: шаги инфинитивом («Kliknąć przycisk…»), в тексте «należy», «można», «trzeba», возвратные и страдательные формы;
tab — karta; screen — ekran; panel (левая панель) — panel; button — przycisk; checkbox — pole wyboru; option — opcja;
logs — logi (как «Logi:» в интерфейсе), log (m.); batch send — wysyłka wsadowa (как в UI), partia; batch (Kafka) — batch;
release page — strona wydań; installer — instalator; build — build (m.: buildu); support request — zgłoszenie do pomocy technicznej;
support — pomoc techniczna; timestamp — znacznik czasu; tombstone — tombstone; listener — listener; handshake — handshake;
policy — zasady (pl., как «Zasady organizacji» в UI); purge — wyczyścić, czyszczenie (как в UI: Wyczyść);
to commit — zatwierdzić, zatwierdzanie (как в UI: Zatwierdź); commit (сущ.) — zatwierdzenie; committed offset — zatwierdzony offset;
экран Producer — «ekran Producer», название не склоняется; macOS menu Help — menu Pomoc; Keychain — pęk kluczy;
Credential Manager — Menedżer poświadczeń; lock icon — ikona kłódki;
метки — только в именительном: «w polu [[ui:…]] wpisać…», «przycisk [[ui:…]]», «sekcja [[ui:…]]»;
read-only connection — połączenie tylko do odczytu (с меткой: «w połączeniach z włączoną opcją [[ui:common.readOnly]]»).
Глава «Asystenci LLM (MCP)»: gateway — bramka; snippet — fragment (konfiguracji); local socket — lokalne gniazdo; named pipe — potok nazwany; token — token; indicator — wskaźnik; badge `LLM` — znaczek.

## Слова руководства (pt-BR)

Термины, которые часто встречаются в `docs/guide/pt-BR` и отсутствуют в словаре интерфейса:
обращение — você, по возможности без местоимения или безличными оборотами («É possível…»); бразильская норма: arquivo, tela, excluir, usuário;
app — aplicativo (как в UI; приложения-потребители группы — тоже aplicativos); tab — aba; screen — tela; panel (левая панель) — painel;
button — botão; checkbox — caixa de seleção; option — opção; field — campo; section — seção; form — formulário; template — modelo;
history — histórico; logs — logs (m., arquivos de log); batch send — envio em lote (как в UI), lote; batch (Kafka) — batch;
release page — página de releases; installer — instalador; build — build (m.); support request — solicitação de suporte; support — suporte;
timestamp — timestamp; tombstone — tombstone; listener — listener; handshake — handshake; policy — política (как в UI);
browse (просмотр сообщений) — navegação, navegar pelas mensagens; purge — limpar, limpeza (как в UI: Limpar);
to commit — fazer commit; committed offset — offset com commit; timeout — tempo limite; lock icon — ícone de cadeado;
macOS menu Help — menu Ajuda; Keychain — Chaves; Credential Manager — Gerenciador de Credenciais;
метки полей — «em [[ui:…]], digite/escolha…», «o campo [[ui:…]]», «a seção [[ui:…]]», «a tela [[ui:…]]»;
read-only connection — conexão somente leitura (с меткой: «nas conexões com a opção [[ui:common.readOnly]] ativada»).
Глава «Assistentes LLM (MCP)»: gateway — gateway; snippet — trecho; named pipe — pipe nomeado; token — token; indicator — indicador; badge `LLM` — indicativo.

## Слова руководства (zh-CN)

Термины, которые часто встречаются в `docs/guide/zh-CN` и отсутствуют в словаре интерфейса:
tab — 标签页; screen — 页面 (как «“许可证”页面» в UI); panel (левая панель) — 面板; button — 按钮; checkbox — 复选框;
field — 字段; form — 表单; section — 部分; header bar панели — 标题栏; logs — 日志 (日志文件); batch send — 批量发送;
batch (Kafka) — 批次; release page — 发布页面; release notes — 发行说明; installer — 安装程序; build — build (латиницей, как в UI и в `errors.updates.notConfigured`; существительное не переводить, глагол — 构建);
support request — 支持请求; support — 技术支持; timestamp — 时间戳; tombstone — tombstone (при первом упоминании «tombstone（墓碑消息）»);
listener — 监听器; handshake — 握手; policy — 策略 (как в UI); browse — 浏览; purge — 清空 (как в UI);
to commit — 提交; committed offset — 已提交的偏移量; lag в прозе — 延迟 / 消费延迟; lock icon — 锁形图标;
point at (наведение) — 将指针悬停在…上; macOS menu Help — “帮助”菜单; Keychain — 钥匙串; Credential Manager — 凭据管理器;
Secret Service — Secret Service; prod connection — 生产环境连接 (`prod` — только как значение окружения); prod mode — 生产模式; Beta build — Beta build; копия при пакетной отправке — 份 / 份序号 (не 副本);
метки — вплотную к иероглифам, но если значение начинается или кончается латиницей или цифрой («Bootstrap 服务器», «Producer»,
«Build», «prod (Pro)»), с этой стороны — пробел («在 [[ui:connections.form.bootstrap]]中»); если значение кончается или начинается
пунктуацией («…», «（…）», «。», «：»), метка берётся в кавычки “[[ui:…]]” или рядом ставится полноширинная пунктуация («点击[[ui:connections.tls.pick]]，然后…»);
read-only connection — 只读连接 (с меткой: «在[[ui:common.readOnly]]连接上»).
Глава «LLM 助手（MCP）»: gateway — 网关; snippet — 片段; local socket — 本地套接字; named pipe — 命名管道; token — 令牌; indicator — 指示器; badge `LLM` — 标记; метки со скобками в значении (“LLM 助手（MCP）”, “LLM 访问（仅元数据）”) — всегда в кавычках “[[ui:…]]”.

## Слова руководства (zh-TW)

Термины, которые часто встречаются в `docs/guide/zh-TW` и отсутствуют в словаре интерфейса (тайваньская норма, перевод с en, не конвертация zh-CN):
click — 按一下 (повтор в той же фразе — 按); right-click — 按右鍵; drag — 拖曳; tab — 分頁; screen — 畫面 (как «「授權」畫面» в UI);
panel (левая панель) — 面板; header bar панели — 標題列; button — 按鈕; checkbox — 核取方塊; select/clear a box — 勾選 / 取消勾選;
field — 欄位; form — 表單; section — 區段; menu — 選單; Start menu — 「開始」功能表; dialog — 對話框; row — 列; column — 欄;
app — 應用程式; installer — 安裝程式; package — 套件; release page — 發行頁面; release notes — 版本資訊; build — build (латиницей, как в UI и в `errors.updates.notConfigured`; существительное не переводить, глагол — 建置);
logs — 日誌 (日誌檔案); batch send — 批次傳送; batch (Kafka) — 批次; copy при пакетной отправке — 份 / 份序號 (не 副本);
support request — 支援要求; support — 支援人員; timestamp — 時間戳記; tombstone — tombstone (при первом упоминании «tombstone（墓碑訊息）»);
listener — 監聽器; handshake — 交握; policy — 原則 (как в UI); browse — 瀏覽; purge — 清除 (как в UI); to commit — 提交;
transaction — 交易; port — 連接埠; timeout — 逾時; proxy — Proxy; hex — 十六進位; point at (наведение) — 將指標停在…上;
prod — 正式環境 (prod connection — 正式環境連線, prod mode — 正式環境模式); lock icon — 鎖頭圖示;
macOS: Terminal — 「終端機」, System Settings → Privacy & Security — 「系統設定」→「隱私權與安全性」, Open Anyway — 「強制打開」,
Help menu — 「輔助說明」選單; Keychain — 鑰匙圈; Credential Manager — 認證管理員; Secret Service — Secret Service;
метки — вплотную к тексту; если значение кончается или начинается пунктуацией («…», «（…）», «。», «：», «」»), метка берётся
в 「[[ui:…]]」 или рядом ставится полноширинная пунктуация («按一下[[ui:connections.tls.pick]]，選擇檔案»); значение, которое
кончается на «：» ([[ui:about.dataDir]], [[ui:about.logDir]]), — в 「」 без пробела («「[[ui:about.dataDir]]」後顯示的是…»); open — 開啟 (как в UI;
системные строки macOS «強制打開», «打開» — как в системе); compacted topic — 經過 Compaction 的主題;
read-only connection — 唯讀連線 (с меткой: «在[[ui:common.readOnly]]連線上»).
Глава «LLM 助理（MCP）»: gateway — 閘道; snippet — 片段; local socket — 本機通訊端; named pipe — 具名管道; token — 權杖; indicator — 指示器; badge `LLM` — 標示; метки со скобками в значении (「LLM 助理（MCP）」, 「LLM 存取（僅中繼資料）」) — всегда в 「[[ui:…]]」.

## Слова руководства (ja)

Термины, которые часто встречаются в `docs/guide/ja` и отсутствуют в словаре интерфейса (です/ます, без пробелов, кавычки 「」):
click — クリック; right-click — 右クリック; drag / drop — ドラッグ / ドロップ; tab — タブ; screen — 画面 (как «「ライセンス」画面» в UI);
panel (левая панель) — パネル; header bar панели — ヘッダー; button — ボタン; checkbox — チェックボックス; select / clear a box — 選択 / チェックを外す;
field — フィールド; form — フォーム; section — セクション; menu — メニュー; Start menu — スタートメニュー; dialog — ダイアログ; row — 行; column — 列;
app — アプリ; installer — インストーラー; package — パッケージ; release page — リリースページ; release notes — リリースノート;
build — ビルド (как в `errors.updates.notConfigured` и подписи [[ui:about.build]]); logs — ログ (ログファイル);
batch send — 一括送信; batch (Kafka) — バッチ; copies при пакетной отправке — コピー / コピー数; support request — サポートへの問い合わせ;
support — サポート / サポート担当者; timestamp — タイムスタンプ; tombstone — tombstone (латиницей); delete marker — 削除マーカー;
listener — リスナー; handshake — ハンドシェイク; policy — ポリシー (как в UI); browse — 閲覧; purge — パージ (как в UI); to commit — コミット;
transaction — トランザクション; port — ポート; timeout — タイムアウト; proxy — プロキシ; hex — 16進数; point at (наведение) — ポインターを合わせる;
interface — インターフェイス; prod — 本番 (prod connection — 本番接続, prod mode — 本番モード; `prod` — только как значение окружения); lock icon — 鍵のアイコン;
macOS: Applications — 「アプリケーション」, Terminal — 「ターミナル」, System Settings → Privacy & Security — 「システム設定」→「プライバシーとセキュリティ」,
Open Anyway — 「このまま開く」, Help menu — 「ヘルプ」メニュー; Keychain — キーチェーン; Credential Manager — 資格情報マネージャー;
SmartScreen «More info / Run anyway» — 「詳細情報」/「実行」; Secret Service — Secret Service;
метки — вплотную к тексту (и к латинице, без пробела); если значение кончается или начинается пунктуацией («…», «（…）», «。», «：», «」»,
«prod (Pro)»), метка берётся в 「[[ui:…]]」 или рядом ставится полноширинная пунктуация («[[ui:connections.form.timeout]]：…»);
сообщения об ошибках в прозе — в 「」 («「[[ui:errors.kafka.timeout]]」は…»), фразы-значения с «。» в конце — в 「」 или в конце абзаца после «：»;
значение, которое кончается на «：» ([[ui:about.dataDir]], [[ui:about.logDir]]) в начале пункта списка, отделяется от текста пробелом.
Глава «LLMアシスタント（MCP）»: gateway — ゲートウェイ; snippet — スニペット; local socket — ローカルソケット; named pipe — 名前付きパイプ; token — トークン; indicator — インジケーター; badge `LLM` — バッジ; метки со скобками в значении (「LLMアシスタント（MCP）」, 「LLMアクセス（メタデータのみ）」) — всегда в 「[[ui:…]]」.

## Слова руководства (ko)

Термины, которые часто встречаются в `docs/guide/ko` и отсутствуют в словаре интерфейса (-습니다, указания -하세요, шаги -합니다, ASCII-пунктуация, кавычки '…'):
click — 클릭; right-click — 오른쪽 클릭; drag / drop — 드래그 / 놓기; tab — 탭; screen — 화면 (как «'라이선스' 화면» в UI); panel (левая панель) — 패널;
header bar панели — 패널 헤더; button — 버튼; checkbox — 확인란; select / clear a box — 선택 / 선택 해제; field — 필드; form — 양식 (как в UI: 양식 지우기);
section — 섹션; area (Batch send, Group and topics) — 영역; menu — 메뉴; Start menu — 시작 메뉴; dialog — 대화 상자; row — 행; column — 열;
app — 앱; installer — 설치 프로그램; package — 패키지; release page — 릴리스 페이지; release notes — 릴리스 노트; build — 빌드 (как в `errors.updates.notConfigured`;
подпись [[ui:about.build]] — «Build»); logs — 로그 (로그 파일); batch send — 일괄 전송; batch (Kafka) — 배치; copies — 사본 / 사본 수;
support request — 지원 요청; support — 지원 담당자; timestamp — 타임스탬프; tombstone — tombstone (латиницей); delete marker — 삭제 마커;
listener — 리스너; handshake — 핸드셰이크; transaction — 트랜잭션; port — 포트; timeout — 제한 시간 (ошибка — 제한 시간 초과); proxy — 프록시;
hex — 16진수; point at (наведение) — 포인터를 올리다; lock icon — 자물쇠 아이콘; browse — 조회; purge — 비우기 (как в UI); to commit — 커밋하다;
prod — 운영 환경 (prod connection — 운영 환경 연결, prod mode — 운영 환경 모드; `prod` — только как значение окружения);
macOS: Applications — '응용 프로그램', Terminal — '터미널', System Settings → Privacy & Security — '시스템 설정' → '개인정보 보호 및 보안',
Open Anyway — '확인 없이 열기', Open — '열기', Help menu — '도움말' 메뉴, quarantine flag — 격리 플래그; Keychain — 키체인; Credential Manager — 자격 증명 관리자;
SmartScreen «More info / Run anyway» — '추가 정보' / '실행'; Secret Service — Secret Service;
метки — с пробелами по 띄어쓰기; частица сразу после метки — по последнему слогу её значения в `ko.json` (을/를, 이/가, 은/는, 과/와, 으로/로),
иначе — постоянное слово после метки («[[ui:…]] 버튼을», «[[ui:…]] 필드에», «[[ui:…]] 옵션을», «[[ui:…]] 값을», «[[ui:…]] 항목으로»);
значение, которое кончается или начинается пунктуацией («…», «)», «:», «(»), отделяется пробелом (частицу после него не ставить);
сообщения об ошибках в прозе — в '…' («'[[ui:errors.kafka.timeout]]' 오류는…»), кроме значений с собственными кавычками;
фразы-значения с «.» в конце — после «다음 메시지가 표시됩니다:» в конце предложения.
Глава «LLM 어시스턴트(MCP)»: gateway — 게이트웨이; snippet — 스니펫; local socket — 로컬 소켓; named pipe — 명명된 파이프; token — 토큰; indicator — 표시기; badge `LLM` — 표시; значения-предложения (“…입니다”, “…했습니다”) после метки — частица «라고» («[[ui:settings.mcp.policy]]라고 표시됩니다»); значения со скобкой в конце (“LLM 접근(메타데이터만)”, “LLM 어시스턴트(MCP)”) — после метки постоянное слово («[[ui:connections.form.llmAccess]] 확인란»).
