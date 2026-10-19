use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::storage::fs::{read_json, write_json};
use crate::sync::RwLockExt;

pub use crate::conn::profile::{ConnectionProfile, Environment, validate_profile};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub order: i64,
    /// Поля из более новой версии Glafira: сохраняются при записи как есть.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub(crate) fn connection_not_found(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.connections.notFound",
        "Connection {{id}} not found",
        id = id
    )
}

const CURRENT_SCHEMA_VERSION: u32 = 1;
const FOLDER_NAME_MAX_CHARS: usize = 100;

/// Папки и подключения читаются по одному: запись, которую эта версия не понимает
/// (например, новое окружение или механизм аутентификации), не ломает весь файл.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsFile {
    schema_version: u32,
    #[serde(default)]
    folders: Vec<serde_json::Value>,
    connections: Vec<serde_json::Value>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// Список для записи: сначала разобранные записи, затем нечитаемые — как были в файле.
struct WithRaw<'a, T>(&'a [T], &'a [serde_json::Value]);

impl<T: Serialize> Serialize for WithRaw<'_, T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.0.len() + self.1.len()))?;
        for item in self.0 {
            seq.serialize_element(item)?;
        }
        for item in self.1 {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

/// Запись файла без копирования данных кэша.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsFileRef<'a> {
    schema_version: u32,
    folders: WithRaw<'a, ConnectionFolder>,
    connections: WithRaw<'a, ConnectionProfile>,
    #[serde(flatten)]
    extra: &'a serde_json::Map<String, serde_json::Value>,
}

/// Содержимое `connections.json` в памяти: подключения, папки и неизвестные поля файла.
///
/// `raw_connections` и `raw_folders` — записи, которые эта версия не смогла разобрать
/// (из более новой Glafira). В приложении они не показываются, `check_tree` их не видит,
/// ссылки на нечитаемую папку считаются корнем; при записи они возвращаются в файл без изменений.
#[derive(Debug, Clone, Default)]
pub(crate) struct StoreData {
    pub connections: Vec<ConnectionProfile>,
    pub folders: Vec<ConnectionFolder>,
    pub extra: serde_json::Map<String, serde_json::Value>,
    pub raw_connections: Vec<serde_json::Value>,
    pub raw_folders: Vec<serde_json::Value>,
}

/// Разбирает записи по одной; нечитаемые уходят в `raw` с предупреждением в журнале.
/// В журнал пишутся только номер и id записи: текст ошибки serde может содержать значения полей.
fn parse_entries<T: serde::de::DeserializeOwned>(
    kind: &str,
    values: Vec<serde_json::Value>,
    raw: &mut Vec<serde_json::Value>,
) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    for (i, v) in values.into_iter().enumerate() {
        match T::deserialize(&v) {
            Ok(item) => out.push(item),
            Err(_) => {
                let id = v.get("id").and_then(|id| id.as_str()).unwrap_or("?");
                tracing::warn!(
                    "connections.json: {kind} #{i} (id {id}) is not supported by this version and is hidden; it is kept in the file"
                );
                raw.push(v);
            }
        }
    }
    out
}

pub(crate) fn folder_not_found(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.folders.notFound",
        "Folder {{id}} not found",
        id = id
    )
}

/// Проверяет имя папки: непустое после trim и не длиннее 100 символов.
pub(crate) fn validate_folder_name(name: &str) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.folders.nameEmpty",
            "Enter a folder name"
        ));
    }
    if name.chars().count() > FOLDER_NAME_MAX_CHARS {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.folders.nameTooLong",
            "The folder name is longer than 100 characters"
        ));
    }
    Ok(())
}

fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Имя, не совпадающее (без учёта регистра) ни с одним из `taken`: при совпадении добавляется
/// суффикс « (2)», « (3)»…; основа укорачивается, чтобы имя уложилось в 100 символов.
pub(crate) fn unique_name(taken: &[&str], wanted: &str) -> String {
    let is_taken = |name: &str| {
        let name = name.to_lowercase();
        taken.iter().any(|t| t.trim().to_lowercase() == name)
    };
    let wanted = truncate_chars(wanted, FOLDER_NAME_MAX_CHARS);
    if !is_taken(wanted) {
        return wanted.to_string();
    }
    (2..)
        .map(|n| {
            let suffix = format!(" ({n})");
            let base = truncate_chars(wanted, FOLDER_NAME_MAX_CHARS - suffix.chars().count());
            format!("{}{suffix}", base.trim_end())
        })
        .find(|candidate| !is_taken(candidate))
        .expect("suffix sequence is infinite")
}

/// Инварианты дерева: ссылки на папки существуют, циклов нет,
/// имена папок валидны и уникальны среди соседей без учёта регистра.
pub(crate) fn check_tree(data: &StoreData) -> AppResult<()> {
    use std::collections::{HashMap, HashSet};

    let by_id: HashMap<&str, &ConnectionFolder> =
        data.folders.iter().map(|f| (f.id.as_str(), f)).collect();
    for c in &data.connections {
        if let Some(fid) = c.folder_id.as_deref()
            && !by_id.contains_key(fid)
        {
            return Err(folder_not_found(fid));
        }
    }
    for f in &data.folders {
        if let Some(pid) = f.parent_id.as_deref()
            && !by_id.contains_key(pid)
        {
            return Err(folder_not_found(pid));
        }
    }
    for f in &data.folders {
        let mut seen = HashSet::new();
        let mut cur = Some(f);
        while let Some(node) = cur {
            if !seen.insert(node.id.as_str()) {
                return Err(app_err!(
                    ErrorCode::InvalidInput,
                    "errors.folders.cycle",
                    "A folder cannot be moved into itself or its subfolder"
                ));
            }
            cur = node
                .parent_id
                .as_deref()
                .and_then(|p| by_id.get(p).copied());
        }
    }
    let mut names = HashSet::new();
    for f in &data.folders {
        validate_folder_name(&f.name)?;
        let name = f.name.trim();
        if !names.insert((f.parent_id.as_deref(), name.to_lowercase())) {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.folders.nameTaken",
                "A folder named “{{name}}” already exists here",
                name = name
            ));
        }
    }
    Ok(())
}

/// Приводит прочитанный файл к допустимому виду без ошибок:
/// старый файл получает `order` по порядку списка, висячие ссылки и циклы уходят в корень.
fn repair_on_read(data: &mut StoreData) {
    use std::collections::{HashMap, HashSet};

    if data.connections.len() > 1 && data.connections.iter().all(|c| c.order == 0) {
        for (i, c) in data.connections.iter_mut().enumerate() {
            c.order = i as i64;
        }
    }
    let ids: HashSet<String> = data.folders.iter().map(|f| f.id.clone()).collect();
    for c in &mut data.connections {
        if c.folder_id.as_ref().is_some_and(|id| !ids.contains(id)) {
            c.folder_id = None;
        }
    }
    for f in &mut data.folders {
        if f.parent_id.as_ref().is_some_and(|id| !ids.contains(id)) {
            f.parent_id = None;
        }
    }
    // Цикл из правленного вручную файла: в корень уходит первая папка, лежащая на цикле
    // (путь вверх возвращается к ней самой); папки, лишь ведущие в цикл, остаются на месте.
    for i in 0..data.folders.len() {
        let parents: HashMap<&str, Option<&str>> = data
            .folders
            .iter()
            .map(|f| (f.id.as_str(), f.parent_id.as_deref()))
            .collect();
        let start = data.folders[i].id.as_str();
        let mut seen = HashSet::new();
        let mut cur = parents.get(start).copied().flatten();
        let mut on_cycle = false;
        while let Some(id) = cur {
            if id == start {
                on_cycle = true;
                break;
            }
            if !seen.insert(id) {
                break;
            }
            cur = parents.get(id).copied().flatten();
        }
        if on_cycle {
            data.folders[i].parent_id = None;
        }
    }
    // Имена: trim, пустое — id папки, не длиннее 100 символов, уникальны среди соседей.
    let mut taken: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for f in &mut data.folders {
        let trimmed = f.name.trim();
        let wanted = if trimmed.is_empty() {
            f.id.trim()
        } else {
            trimmed
        };
        let wanted = truncate_chars(wanted, FOLDER_NAME_MAX_CHARS)
            .trim_end()
            .to_string();
        let siblings = taken.entry(f.parent_id.clone()).or_default();
        let refs: Vec<&str> = siblings.iter().map(String::as_str).collect();
        let name = unique_name(&refs, &wanted);
        siblings.push(name.clone());
        f.name = name;
    }
}

pub struct ConnectionStore {
    path: PathBuf,
    cache: RwLock<StoreData>,
}

impl ConnectionStore {
    pub fn open(dir: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("connections.json");
        let mut data = match read_json::<ConnectionsFile>(&path)? {
            Some(file) if file.schema_version > CURRENT_SCHEMA_VERSION => {
                return Err(crate::storage::fs::newer_schema_error(
                    &path,
                    file.schema_version,
                ));
            }
            Some(file) => {
                let mut raw_connections = Vec::new();
                let mut raw_folders = Vec::new();
                StoreData {
                    connections: parse_entries(
                        "connection",
                        file.connections,
                        &mut raw_connections,
                    ),
                    folders: parse_entries("folder", file.folders, &mut raw_folders),
                    extra: file.extra,
                    raw_connections,
                    raw_folders,
                }
            }
            None => StoreData::default(),
        };
        repair_on_read(&mut data);
        Ok(Self {
            path,
            cache: RwLock::new(data),
        })
    }

    pub fn list(&self) -> Vec<ConnectionProfile> {
        self.cache.read_safe().connections.clone()
    }

    pub fn list_folders(&self) -> Vec<ConnectionFolder> {
        self.cache.read_safe().folders.clone()
    }

    pub fn get(&self, id: &str) -> AppResult<ConnectionProfile> {
        self.cache
            .read_safe()
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| connection_not_found(id))
    }

    /// Копия всех данных хранилища (для расчётов без записи, например предпросмотра импорта).
    // Вызывается предпросмотром импорта (команды импорта вернутся позже).
    #[allow(dead_code)]
    pub(crate) fn snapshot(&self) -> StoreData {
        self.cache.read_safe().clone()
    }

    /// Изменяет данные одной транзакцией: `f` работает с копией, затем проверяются
    /// инварианты дерева и пишется файл; кэш заменяется только после успешной записи.
    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut StoreData) -> AppResult<R>) -> AppResult<R> {
        let mut cache = self.cache.write_safe();
        let mut next = cache.clone();
        let result = f(&mut next)?;
        check_tree(&next)?;
        self.persist(&next)?;
        *cache = next;
        Ok(result)
    }

    /// Создаёт или обновляет подключение и возвращает его в сохранённом виде.
    ///
    /// Новое подключение кладётся в конец папки `folder_id` входящего профиля.
    /// У существующего `folder_id` и `order` берутся из сохранённого, входящие значения
    /// игнорируются: место подключения меняют только команды перемещения. Входящий `extra`
    /// игнорируется всегда: неизвестные поля пишет только сама Glafira (существующий профиль
    /// сохраняет свои, новый получает пустые), поэтому через них нельзя записать секрет в файл.
    pub fn upsert(&self, mut profile: ConnectionProfile) -> AppResult<ConnectionProfile> {
        validate_profile(&profile)?;
        self.update(|data| {
            match data.connections.iter_mut().find(|c| c.id == profile.id) {
                Some(slot) => {
                    profile.extra = std::mem::take(&mut slot.extra);
                    profile.folder_id = slot.folder_id.take();
                    profile.order = slot.order;
                    *slot = profile.clone();
                }
                None => {
                    profile.extra = Default::default();
                    profile.order = data
                        .connections
                        .iter()
                        .filter(|c| c.folder_id == profile.folder_id)
                        .map(|c| c.order + 1)
                        .max()
                        .unwrap_or(0);
                    data.connections.push(profile.clone());
                }
            }
            Ok(profile)
        })
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        self.update(|data| {
            if !data.connections.iter().any(|c| c.id == id) {
                return Err(connection_not_found(id));
            }
            data.connections.retain(|c| c.id != id);
            Ok(())
        })
    }

    fn persist(&self, data: &StoreData) -> AppResult<()> {
        write_json(
            &self.path,
            &ConnectionsFileRef {
                schema_version: CURRENT_SCHEMA_VERSION,
                folders: WithRaw(&data.folders, &data.raw_folders),
                connections: WithRaw(&data.connections, &data.raw_connections),
                extra: &data.extra,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{SslMode, sample_profile};
    use crate::error::ErrorCode;

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        store.upsert(sample_profile("a")).unwrap();
        let mut b = sample_profile("b");
        b.pg.ssl_mode = SslMode::VerifyFull;
        let b = store.upsert(b).unwrap();

        let reopened = ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().len(), 2);
        assert_eq!(reopened.get("b").unwrap(), b);
        assert_eq!(reopened.get("b").unwrap().pg.ssl_mode, SslMode::VerifyFull);
    }

    #[test]
    fn upsert_rejects_invalid_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        let mut p = sample_profile("a");
        p.name = "  ".into();
        assert_eq!(store.upsert(p).unwrap_err().code, ErrorCode::InvalidInput);
        let mut p = sample_profile("a");
        p.hosts.clear();
        assert_eq!(store.upsert(p).unwrap_err().code, ErrorCode::InvalidInput);
        assert!(store.list().is_empty());
    }

    #[test]
    fn upsert_replaces_and_delete_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        store.upsert(sample_profile("a")).unwrap();
        let mut a2 = sample_profile("a");
        a2.name = "renamed".into();
        store.upsert(a2).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.get("a").unwrap().name, "renamed");
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(store.delete("a").unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn file_contains_schema_version_and_no_password_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        store.upsert(sample_profile("a")).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(raw.contains("\"schemaVersion\": 1"));
        assert!(!raw.to_lowercase().contains("password"));
    }

    #[test]
    fn migration_keeps_connections_in_root_with_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r##"{"schemaVersion":1,"connections":[
          {"id":"a","name":"A","color":"#fff","environment":"dev","readOnly":false,"kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"}},
          {"id":"b","name":"B","color":"#fff","environment":"dev","readOnly":false,"kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"}}]}"##,
        )
        .unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        let list = store.list();
        assert_eq!(
            list.iter()
                .map(|c| (c.id.as_str(), c.folder_id.clone(), c.order))
                .collect::<Vec<_>>(),
            vec![("a", None, 0), ("b", None, 1)]
        );
        assert!(store.list_folders().is_empty());
    }

    #[test]
    fn unknown_fields_survive_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r##"{"schemaVersion":1,"future":{"x":1},"folders":[{"id":"f","name":"F","parentId":null,"order":0,"icon":"star"}],
           "connections":[{"id":"a","name":"A","color":"#fff","environment":"dev","readOnly":false,"kind":"server",
           "hosts":[{"host":"h","port":1}],"pg":{"username":"u"},"folderId":"f","order":0,"pinned":true}]}"##,
        )
        .unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        let mut p = store.get("a").unwrap();
        p.name = "A2".into();
        // Входящие неизвестные поля игнорируются: остаются сохранённые.
        p.extra.insert("pinned".into(), serde_json::json!(false));
        store.upsert(p).unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("connections.json")).unwrap())
                .unwrap();
        assert_eq!(raw["future"]["x"], 1);
        assert_eq!(raw["folders"][0]["icon"], "star");
        assert_eq!(raw["connections"][0]["pinned"], true);
        assert_eq!(raw["connections"][0]["name"], "A2");
        assert_eq!(raw["connections"][0]["folderId"], "f");
    }

    #[test]
    fn unparseable_entries_are_kept_but_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let qa = serde_json::json!({"id":"q","name":"QA","color":"#fff","environment":"qa","readOnly":false,
            "kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"},"folderId":null,"order":0,"pinned":true});
        let raw_folder =
            serde_json::json!({"id":"rf","name":{"smart":"x"},"parentId":null,"order":1});
        let file = serde_json::json!({"schemaVersion":1,
            "folders":[raw_folder, {"id":"f","name":"F","parentId":"rf","order":0}],
            "connections":[qa, {"id":"a","name":"A","color":"#fff","environment":"dev","readOnly":false,
                "kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"},"folderId":"rf","order":1}]});
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let store = ConnectionStore::open(dir.path()).unwrap();
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
        // Ссылки на нечитаемую папку — корень.
        assert_eq!(list[0].folder_id, None);
        let folders = store.list_folders();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].parent_id, None);

        let mut a = store.get("a").unwrap();
        a.name = "A2".into();
        store.upsert(a).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let conns = written["connections"].as_array().unwrap();
        assert_eq!(conns.len(), 2);
        assert!(conns.contains(&qa));
        assert!(conns.iter().any(|c| c["id"] == "a" && c["name"] == "A2"));
        let fl = written["folders"].as_array().unwrap();
        assert_eq!(fl.len(), 2);
        assert!(fl.contains(&raw_folder));

        // После перезапуска то же самое: запись ничего не теряет.
        let reopened = ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().len(), 1);
        reopened.delete("a").unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["connections"], serde_json::json!([qa]));
    }

    #[test]
    fn new_connection_goes_to_end_of_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        store.upsert(sample_profile("a")).unwrap();
        store.upsert(sample_profile("b")).unwrap();
        assert_eq!(store.get("b").unwrap().order, 1);
    }

    #[test]
    fn dangling_folder_reference_is_rejected_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        let mut p = sample_profile("a");
        p.folder_id = Some("missing".into());
        let e = store.upsert(p).unwrap_err();
        assert_eq!(e.key, "errors.folders.notFound");
    }

    fn folder(id: &str, name: &str, parent: Option<&str>) -> ConnectionFolder {
        ConnectionFolder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(String::from),
            order: 0,
            extra: Default::default(),
        }
    }

    #[test]
    fn check_tree_rejects_bad_folders() {
        let data = |folders: Vec<ConnectionFolder>| StoreData {
            folders,
            ..Default::default()
        };
        assert!(
            check_tree(&data(vec![
                folder("a", "A", None),
                folder("b", "a", Some("a"))
            ]))
            .is_ok()
        );
        for (folders, key) in [
            (
                vec![folder("a", "A", Some("b")), folder("b", "B", Some("a"))],
                "errors.folders.cycle",
            ),
            (vec![folder("a", "A", Some("a"))], "errors.folders.cycle"),
            (vec![folder("a", "A", Some("x"))], "errors.folders.notFound"),
            (vec![folder("a", "  ", None)], "errors.folders.nameEmpty"),
            (
                vec![folder("a", &"я".repeat(101), None)],
                "errors.folders.nameTooLong",
            ),
            (
                vec![folder("a", "Prod", None), folder("b", " prod ", None)],
                "errors.folders.nameTaken",
            ),
        ] {
            assert_eq!(check_tree(&data(folders)).unwrap_err().key, key);
        }
        assert!(check_tree(&data(vec![folder("a", &"я".repeat(100), None)])).is_ok());
    }

    #[test]
    fn dangling_references_and_cycles_go_to_root_on_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r##"{"schemaVersion":1,"folders":[
              {"id":"f","name":"F","parentId":"gone"},
              {"id":"x","name":"X","parentId":"y"},{"id":"y","name":"Y","parentId":"x"}],
              "connections":[{"id":"a","name":"A","color":"#fff","environment":"dev","readOnly":false,
              "kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"},"folderId":"gone"}]}"##,
        )
        .unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(store.get("a").unwrap().folder_id, None);
        let folders = store.list_folders();
        assert_eq!(folders[0].parent_id, None);
        check_tree(&store.cache.read_safe()).unwrap();
        store.delete("a").unwrap();
    }

    #[test]
    fn folder_names_are_repaired_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let long = "я".repeat(101);
        std::fs::write(
            dir.path().join("connections.json"),
            format!(
                r##"{{"schemaVersion":1,"folders":[
                  {{"id":"e","name":"   "}},{{"id":"l","name":"{long}"}},
                  {{"id":"p1","name":"Prod"}},{{"id":"p2","name":" prod "}},{{"id":"p3","name":"PROD"}},
                  {{"id":"c","name":"prod","parentId":"p1"}}],
                  "connections":[]}}"##
            ),
        )
        .unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        check_tree(&store.cache.read_safe()).unwrap();
        let names: Vec<_> = store.list_folders().into_iter().map(|f| f.name).collect();
        assert_eq!(names[0], "e");
        assert_eq!(names[1], "я".repeat(100));
        assert_eq!(&names[2..], ["Prod", "prod (2)", "PROD (3)", "prod"]);
        store.upsert(sample_profile("a")).unwrap();
    }

    #[test]
    fn unique_name_adds_suffix_case_insensitively() {
        assert_eq!(unique_name(&[], "A"), "A");
        assert_eq!(unique_name(&["a"], "A"), "A (2)");
        assert_eq!(unique_name(&["A", "a (2)"], "A"), "A (3)");
        let long = "x".repeat(100);
        let n = unique_name(&[long.as_str()], &long);
        assert_eq!(n.chars().count(), 100);
        assert!(n.ends_with(" (2)"));
    }

    #[test]
    fn upsert_of_existing_profile_keeps_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        store
            .update(|d| {
                d.folders.push(folder("f", "F", None));
                Ok(())
            })
            .unwrap();
        store.upsert(sample_profile("x")).unwrap();
        let mut p = sample_profile("a");
        p.folder_id = Some("f".into());
        let saved = store.upsert(p).unwrap();
        assert_eq!(saved.order, 0);
        let mut edited = saved.clone();
        edited.folder_id = None;
        edited.order = 99;
        edited.name = "renamed".into();
        let back = store.upsert(edited).unwrap();
        assert_eq!(back.folder_id.as_deref(), Some("f"));
        assert_eq!(back.order, 0);
        let stored = store.get("a").unwrap();
        assert_eq!(
            (
                stored.folder_id.as_deref(),
                stored.order,
                stored.name.as_str()
            ),
            (Some("f"), 0, "renamed")
        );
    }

    #[test]
    fn cycle_repair_detaches_only_folders_on_the_cycle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r##"{"schemaVersion":1,"folders":[
              {"id":"z","name":"Z","parentId":"x"},
              {"id":"x","name":"X","parentId":"y"},{"id":"y","name":"Y","parentId":"x"}],
              "connections":[]}"##,
        )
        .unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        let f = store.list_folders();
        assert_eq!(f[0].parent_id.as_deref(), Some("x"));
        let roots = f[1..].iter().filter(|f| f.parent_id.is_none()).count();
        assert_eq!(roots, 1);
        check_tree(&store.cache.read_safe()).unwrap();
    }

    #[test]
    fn unknown_fields_are_not_validated() {
        // Поля из более новой версии с «password»/«secret» в имени не мешают сохранению.
        let mut p = sample_profile("a");
        for key in ["passwordRotationDays", "secretRef", "usesSecretManager"] {
            p.extra.insert(key.into(), serde_json::json!(1));
        }
        assert!(validate_profile(&p).is_ok());
    }

    #[test]
    fn incoming_extra_is_ignored_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path()).unwrap();
        // Новый профиль: неизвестные поля из запроса не пишутся.
        let mut p = sample_profile("a");
        p.extra.insert("pgPassword".into(), serde_json::json!("x"));
        p.extra.insert("pinned".into(), serde_json::json!(true));
        assert!(store.upsert(p).unwrap().extra.is_empty());
        assert!(store.get("a").unwrap().extra.is_empty());
        let raw = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(!raw.contains("pgPassword") && !raw.contains("pinned"));
        // Существующий профиль всегда сохраняет свои неизвестные поля.
        store
            .update(|d| {
                d.connections[0]
                    .extra
                    .insert("passwordRotationDays".into(), serde_json::json!(30));
                Ok(())
            })
            .unwrap();
        let mut edited = store.get("a").unwrap();
        edited.extra.clear();
        edited
            .extra
            .insert("clientSecret".into(), serde_json::json!("x"));
        edited.name = "A2".into();
        let saved = store.upsert(edited).unwrap();
        assert_eq!(saved.name, "A2");
        assert_eq!(
            serde_json::Value::Object(saved.extra),
            serde_json::json!({"passwordRotationDays": 30})
        );
        let raw = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(!raw.contains("clientSecret"));
    }
}
