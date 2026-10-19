//! Файл обмена подключениями (`glafira-postgres-connections`): экспорт без секретов,
//! предпросмотр и применение импорта.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::conn::profile::{ConnectionProfile, Environment, normalize_profile, validate_profile};
use crate::error::{AppResult, ErrorCode};
use crate::i18n::LocalizedText;
use crate::secrets::{
    PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key,
};
use crate::storage::connections::{
    ConnectionFolder, ConnectionStore, StoreData, check_tree, folder_not_found, unique_name,
};
use crate::storage::tree::TreeItemKind;

pub const EXCHANGE_FORMAT: &str = "glafira-postgres-connections";
pub const EXCHANGE_VERSION: u32 = 1;

/// Какие секреты были сохранены у подключения при экспорте. Недостающие в файле флаги — `false`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NeedsPassword {
    pub pg: bool,
    pub pg_key: bool,
    pub patroni: bool,
    pub patroni_key: bool,
}

impl NeedsPassword {
    pub fn any(&self) -> bool {
        self.pg || self.pg_key || self.patroni || self.patroni_key
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeFileOut<'a> {
    pub format: &'a str,
    pub version: u32,
    pub exported_at: String,
    pub app_version: &'a str,
    pub folders: Vec<ConnectionFolder>,
    pub connections: Vec<ExportedConnection>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportedConnection {
    #[serde(flatten)]
    pub profile: ConnectionProfile,
    #[serde(default)]
    pub needs_password: NeedsPassword,
}

/// Итог экспорта для фронтенда.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub connections: usize,
    pub folders: usize,
}

/// Флаг поднимается, только если секрет сохранён и профиль его действительно использует
/// (`ConnectionProfile::uses_secret`, та же матрица, что при подключении и сохранении):
/// устаревшие записи keychain не требуют ввода у получателя.
fn needs_password(secrets: &dyn SecretStore, p: &ConnectionProfile) -> AppResult<NeedsPassword> {
    let check = |field: &str| -> AppResult<bool> {
        if !p.uses_secret(field) {
            return Ok(false);
        }
        Ok(secrets
            .get(&secret_key(&p.id, field))?
            .is_some_and(|v| !v.is_empty()))
    };
    Ok(NeedsPassword {
        pg: check(PG_PASSWORD)?,
        pg_key: check(PG_KEY_PASSWORD)?,
        patroni: check(PATRONI_PASSWORD)?,
        patroni_key: check(PATRONI_KEY_PASSWORD)?,
    })
}

/// Адрес без `user:password@` в authority: пароль, вписанный в URL, в файл обмена не попадает.
fn strip_url_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// Есть ли в адресе `user:password@` (хранить такое в профиле нельзя: секреты только в keychain).
fn has_url_credentials(url: &str) -> bool {
    strip_url_credentials(url) != url
}

/// Адреса Patroni REST API профиля: списки `urls` и `apiUrl` подмен адресов участников.
fn patroni_urls_mut(p: &mut ConnectionProfile) -> impl Iterator<Item = &mut String> {
    p.patroni.iter_mut().flat_map(|pt| {
        pt.urls.iter_mut().chain(
            pt.host_overrides
                .iter_mut()
                .filter_map(|o| o.api_url.as_mut()),
        )
    })
}

/// Собирает файл экспорта: всё дерево или поддерево `root` (сама папка становится корнем).
/// Секреты (keychain и `user:password@` в адресах Patroni) не попадают в файл; неизвестные
/// локальные поля (`extra`) не выносятся.
pub fn build_export(
    store: &ConnectionStore,
    secrets: &dyn SecretStore,
    root: Option<&str>,
    app_version: &'static str,
    now: &str,
) -> AppResult<ExchangeFileOut<'static>> {
    let all_folders = store.list_folders();
    let included: Option<HashSet<String>> = match root {
        None => None,
        Some(root) => {
            if !all_folders.iter().any(|f| f.id == root) {
                return Err(folder_not_found(root));
            }
            let mut set = HashSet::from([root.to_string()]);
            // Дерево без циклов (инвариант хранилища), поэтому достаточно расширять до неподвижной точки.
            loop {
                let before = set.len();
                for f in &all_folders {
                    if f.parent_id.as_deref().is_some_and(|p| set.contains(p)) {
                        set.insert(f.id.clone());
                    }
                }
                if set.len() == before {
                    break;
                }
            }
            Some(set)
        }
    };
    let in_scope = |folder: Option<&str>| match &included {
        None => true,
        Some(set) => folder.is_some_and(|id| set.contains(id)),
    };

    let folders = all_folders
        .into_iter()
        .filter(|f| in_scope(Some(&f.id)))
        .map(|mut f| {
            if Some(f.id.as_str()) == root {
                f.parent_id = None;
            }
            f.extra.clear();
            f
        })
        .collect();

    let connections = store
        .list()
        .into_iter()
        .filter(|p| in_scope(p.folder_id.as_deref()))
        .map(|mut profile| {
            profile.extra.clear();
            // Профили с диска не валидируются: пароль, вписанный в адрес Patroni, не выносим.
            patroni_urls_mut(&mut profile).for_each(|u| *u = strip_url_credentials(u));
            let needs_password = needs_password(secrets, &profile)?;
            Ok(ExportedConnection {
                profile,
                needs_password,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    Ok(ExchangeFileOut {
        format: EXCHANGE_FORMAT,
        version: EXCHANGE_VERSION,
        exported_at: now.to_string(),
        app_version,
        folders,
        connections,
    })
}

/// Подключение файла, которое не будет импортировано, и причина.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub name: String,
    pub reason: LocalizedText,
}

/// Прочитанный файл обмена: папки, подключения (с флагами паролей) и пропущенные подключения.
#[derive(Debug, Clone, Default)]
pub struct ParsedExchange {
    pub folders: Vec<ConnectionFolder>,
    pub connections: Vec<(ConnectionProfile, NeedsPassword)>,
    pub skipped: Vec<Skipped>,
}

fn not_glafira_file() -> crate::error::AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.import.notGlafiraFile",
        "This is not a Glafira connections file"
    )
}

fn invalid_json(error: impl std::fmt::Display) -> crate::error::AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.import.invalidJson",
        "The file is damaged or is not JSON: {{error}}",
        error = error
    )
}

/// Имя подключения для списка пропущенных: из файла, иначе id, иначе номер в списке.
fn display_name(v: &serde_json::Value, index: usize) -> String {
    ["name", "id"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| format!("#{}", index + 1))
}

/// Читает файл обмена. Ошибка — только для чужого или повреждённого файла и более новой версии;
/// неизвестные поля игнорируются, неподдерживаемое или невалидное подключение попадает в `skipped`.
pub fn parse_exchange(bytes: &[u8]) -> AppResult<ParsedExchange> {
    let root: serde_json::Value = serde_json::from_slice(bytes).map_err(invalid_json)?;
    if root.get("format").and_then(|f| f.as_str()) != Some(EXCHANGE_FORMAT) {
        return Err(not_glafira_file());
    }
    let version = root
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(not_glafira_file)?;
    if version > u64::from(EXCHANGE_VERSION) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.import.incompatibleVersion",
            "The file was created by an incompatible version of Glafira — update the app"
        ));
    }
    let list = |key: &str| -> AppResult<Vec<serde_json::Value>> {
        match root.get(key) {
            None | Some(serde_json::Value::Null) => Ok(Vec::new()),
            Some(serde_json::Value::Array(items)) => Ok(items.clone()),
            Some(_) => Err(invalid_json(format!("“{key}” is not a list"))),
        }
    };

    let mut seen = HashSet::new();
    let mut folders = Vec::new();
    for v in list("folders")? {
        let mut f: ConnectionFolder = serde_json::from_value(v).map_err(invalid_json)?;
        // Папка без id пропускается (её подключения попадут в цель); из повторов id берётся первая.
        if f.id.trim().is_empty() || !seen.insert(f.id.clone()) {
            continue;
        }
        f.extra.clear();
        let name = f.name.trim();
        f.name = if name.is_empty() { f.id.trim() } else { name }.to_string();
        folders.push(f);
    }

    let mut seen = HashSet::new();
    let mut connections = Vec::new();
    let mut skipped = Vec::new();
    for (i, v) in list("connections")?.into_iter().enumerate() {
        let name = display_name(&v, i);
        let mut c: ExportedConnection = match serde_json::from_value(v) {
            Ok(c) => c,
            Err(e) => {
                skipped.push(Skipped {
                    name,
                    reason: ltext!(
                        "errors.import.unsupportedConnection",
                        "Not supported by this version of Glafira (update the app): {{error}}",
                        error = e
                    ),
                });
                continue;
            }
        };
        // Неизвестные поля файла не переносятся в хранилище.
        c.profile.extra.clear();
        // Профиль проходит ту же нормализацию и проверку, что и при сохранении формы.
        normalize_profile(&mut c.profile);
        if let Err(e) = validate_profile(&c.profile) {
            skipped.push(Skipped {
                name,
                reason: e.text(),
            });
            continue;
        }
        if patroni_urls_mut(&mut c.profile).any(|u| has_url_credentials(u)) {
            skipped.push(Skipped {
                name,
                reason: ltext!(
                    "errors.import.secretInUrl",
                    "The Patroni API address contains a user name or password: remove it from the address"
                ),
            });
            continue;
        }
        if !seen.insert(c.profile.id.clone()) {
            skipped.push(Skipped {
                name,
                reason: ltext!(
                    "errors.import.duplicateId",
                    "Duplicate connection id in the file"
                ),
            });
            continue;
        }
        connections.push((c.profile, c.needs_password));
    }
    Ok(ParsedExchange {
        folders,
        connections,
        skipped,
    })
}

pub struct ImportOptions<'a> {
    /// Папка, под которую попадают корневые элементы файла; `None` — корень.
    pub target: Option<&'a str>,
    pub update_existing: bool,
    pub is_pro: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub kind: TreeItemKind,
    pub id: String,
    pub name: String,
    /// Имена папок от корня до места элемента (сам элемент не входит).
    pub path: Vec<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub added: Vec<PreviewItem>,
    pub updated: Vec<PreviewItem>,
    pub unchanged: Vec<PreviewItem>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<LocalizedText>,
    pub needs_passwords: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportPlan {
    pub next: StoreData,
    pub preview: ImportPreview,
}

/// Имена папок от корня до `folder` включительно.
fn path_of(folders: &[ConnectionFolder], folder: Option<&str>) -> Vec<String> {
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = folder;
    while let Some(id) = cur {
        if !seen.insert(id) {
            break;
        }
        let Some(f) = folders.iter().find(|f| f.id == id) else {
            break;
        };
        path.push(f.name.clone());
        cur = f.parent_id.as_deref();
    }
    path.reverse();
    path
}

/// Папки файла в порядке «родитель раньше потомка» (внутри уровня — по `order`).
/// Папка с родителем не из файла — корневая в файле; цикл разрывается на первой оставшейся папке.
fn parents_first(folders: &[ConnectionFolder]) -> Vec<(&ConnectionFolder, bool)> {
    let ids: HashSet<&str> = folders.iter().map(|f| f.id.as_str()).collect();
    let mut rest: Vec<&ConnectionFolder> = folders.iter().collect();
    rest.sort_by_key(|f| f.order);
    let mut done: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    while !rest.is_empty() {
        let ready = |f: &&ConnectionFolder| match f.parent_id.as_deref() {
            None => true,
            Some(p) => !ids.contains(p) || done.contains(p),
        };
        let (mut now, later): (Vec<_>, Vec<_>) = rest.into_iter().partition(ready);
        let mut forced_root = None;
        if now.is_empty() {
            // Цикл в файле: первая оставшаяся папка становится корневой.
            forced_root = Some(later[0].id.as_str());
            now.push(later[0]);
        }
        rest = later
            .into_iter()
            .filter(|f| Some(f.id.as_str()) != forced_root)
            .collect();
        for f in now {
            let root = Some(f.id.as_str()) == forced_root
                || f.parent_id.as_deref().is_none_or(|p| !ids.contains(p));
            out.push((f, root));
            done.insert(f.id.as_str());
        }
    }
    out
}

/// Хотя бы один указанный файл сертификата (PG или Patroni API) отсутствует на этом компьютере.
fn misses_files(p: &ConnectionProfile, file_exists: &dyn Fn(&str) -> bool) -> bool {
    let pg_cert = p.pg.tls.client_cert.as_ref();
    let api_tls = p.patroni.as_ref().and_then(|x| x.tls.as_ref());
    let api_cert = api_tls.and_then(|t| t.client_cert.as_ref());
    [
        p.pg.tls.ca_path.as_deref(),
        pg_cert.map(|c| c.certificate_path.as_str()),
        pg_cert.and_then(|c| c.key_path.as_deref()),
        api_tls.and_then(|t| t.ca_path.as_deref()),
        api_cert.map(|c| c.certificate_path.as_str()),
        api_cert.and_then(|c| c.key_path.as_deref()),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .any(|path| !file_exists(path))
}

/// Настройки подключения без места в дереве и локальных неизвестных полей.
fn same_settings(a: &ConnectionProfile, b: &ConnectionProfile) -> bool {
    let strip = |p: &ConnectionProfile| ConnectionProfile {
        id: String::new(),
        folder_id: None,
        order: 0,
        extra: Default::default(),
        ..p.clone()
    };
    strip(a) == strip(b)
}

/// Строит новое состояние хранилища и предпросмотр импорта. Функция чистая: лицензия и
/// проверка файлов сертификатов приходят снаружи.
pub(crate) fn plan_import(
    data: &StoreData,
    parsed: &ParsedExchange,
    opts: &ImportOptions,
    file_exists: &dyn Fn(&str) -> bool,
) -> AppResult<ImportPlan> {
    let target = opts.target.map(str::to_string);
    if let Some(t) = &target
        && !data.folders.iter().any(|f| &f.id == t)
    {
        return Err(folder_not_found(t));
    }
    let mut next = data.clone();
    let mut preview = ImportPreview {
        skipped: parsed.skipped.clone(),
        ..Default::default()
    };
    // id папок файла совпадают с id в хранилище: существующие берутся как есть, новые создаются с тем же id.
    let file_folders: HashSet<&str> = parsed.folders.iter().map(|f| f.id.as_str()).collect();
    let local_folders: HashSet<&str> = data.folders.iter().map(|f| f.id.as_str()).collect();
    // (вид, id) в порядке появления, чтобы пути в предпросмотре считались по итоговому дереву.
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut unchanged = Vec::new();

    for (f, is_root) in parents_first(&parsed.folders) {
        if local_folders.contains(f.id.as_str()) {
            unchanged.push((TreeItemKind::Folder, f.id.clone()));
            continue;
        }
        let parent = if is_root {
            target.clone()
        } else {
            f.parent_id.clone()
        };
        let siblings: Vec<&str> = next
            .folders
            .iter()
            .filter(|s| s.parent_id == parent)
            .map(|s| s.name.as_str())
            .collect();
        let name = unique_name(&siblings, &f.name);
        let order = next
            .folders
            .iter()
            .filter(|s| s.parent_id == parent)
            .map(|s| s.order + 1)
            .max()
            .unwrap_or(0);
        next.folders.push(ConnectionFolder {
            id: f.id.clone(),
            name,
            parent_id: parent,
            order,
            extra: Default::default(),
        });
        added.push((TreeItemKind::Folder, f.id.clone()));
    }

    let mut connections: Vec<&(ConnectionProfile, NeedsPassword)> =
        parsed.connections.iter().collect();
    connections.sort_by_key(|(p, _)| p.order);
    let mut prod_read_only = 0usize;
    let mut missing_files = 0usize;
    for (incoming, needs) in connections {
        let mut profile = incoming.clone();
        let forced_read_only = !opts.is_pro && profile.environment == Environment::Prod;
        if forced_read_only {
            profile.read_only = true;
        }
        if let Some(slot) = next.connections.iter_mut().find(|c| c.id == profile.id) {
            // Во Free совпадение с файлом «как есть» — тоже «без изменений»: иначе локальный
            // prod без readOnly выглядел бы обновлённым при каждом импорте того же файла.
            let unchanged_settings = same_settings(slot, incoming) || same_settings(slot, &profile);
            if unchanged_settings || !opts.update_existing {
                unchanged.push((TreeItemKind::Connection, profile.id.clone()));
                continue;
            }
            // Место в дереве и неизвестные поля — локальные; пароли получателя не трогаются.
            profile.folder_id = slot.folder_id.clone();
            profile.order = slot.order;
            profile.extra = std::mem::take(&mut slot.extra);
            prod_read_only += usize::from(forced_read_only);
            missing_files += usize::from(misses_files(&profile, file_exists));
            updated.push((TreeItemKind::Connection, profile.id.clone()));
            *slot = profile;
            continue;
        }
        let folder = profile
            .folder_id
            .as_deref()
            .filter(|f| file_folders.contains(f))
            .map(str::to_string)
            .or_else(|| target.clone());
        let siblings: Vec<&str> = next
            .connections
            .iter()
            .filter(|c| c.folder_id == folder)
            .map(|c| c.name.as_str())
            .collect();
        let wanted = profile.name.trim();
        let taken = siblings
            .iter()
            .any(|s| s.trim().to_lowercase() == wanted.to_lowercase());
        let name = if taken {
            unique_name(&siblings, wanted)
        } else {
            profile.name.clone()
        };
        profile.order = next
            .connections
            .iter()
            .filter(|c| c.folder_id == folder)
            .map(|c| c.order + 1)
            .max()
            .unwrap_or(0);
        profile.folder_id = folder;
        profile.name = name;
        profile.extra = Default::default();
        prod_read_only += usize::from(forced_read_only);
        missing_files += usize::from(misses_files(&profile, file_exists));
        if needs.any() {
            preview.needs_passwords += 1;
        }
        added.push((TreeItemKind::Connection, profile.id.clone()));
        next.connections.push(profile);
    }

    if prod_read_only > 0 {
        preview.warnings.push(ltext!(
            "warnings.import.prodReadOnly",
            "{{count}} prod connections will be read-only: changing prod connections requires Pro",
            count = prod_read_only
        ));
    }
    if missing_files > 0 {
        preview.warnings.push(ltext!(
            "warnings.import.missingFiles",
            "Certificate files of {{count}} connections are not found on this computer — check the paths",
            count = missing_files
        ));
    }
    check_tree(&next)?;

    let folders_by_id: HashMap<&str, &ConnectionFolder> =
        next.folders.iter().map(|f| (f.id.as_str(), f)).collect();
    let item = |(kind, id): (TreeItemKind, String)| -> PreviewItem {
        let (name, location) = match kind {
            TreeItemKind::Folder => {
                let f = folders_by_id[id.as_str()];
                (f.name.clone(), f.parent_id.as_deref())
            }
            TreeItemKind::Connection => {
                let c = next
                    .connections
                    .iter()
                    .find(|c| c.id == id)
                    .expect("planned connection exists");
                (c.name.clone(), c.folder_id.as_deref())
            }
        };
        PreviewItem {
            kind,
            path: path_of(&next.folders, location),
            id,
            name,
        }
    };
    preview.added = added.into_iter().map(item).collect();
    preview.updated = updated.into_iter().map(item).collect();
    preview.unchanged = unchanged.into_iter().map(item).collect();
    Ok(ImportPlan { next, preview })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{
        ApiTls, CertFormat, ClientCert, ConnectionKind, HostOverride, PatroniSettings,
        sample_profile,
    };
    use crate::secrets::MemoryStore;

    fn store() -> (tempfile::TempDir, ConnectionStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ConnectionStore::open(d.path()).unwrap();
        (d, s)
    }

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:\\certs\\{p}")
        } else {
            format!("/certs/{p}")
        }
    }

    fn in_folder(id: &str, folder: Option<&str>) -> ConnectionProfile {
        let mut p = sample_profile(id);
        p.folder_id = folder.map(String::from);
        p
    }

    /// Подключение Patroni: адреса API, пользователь и клиентский сертификат PKCS#12 для API.
    fn patroni_profile(id: &str) -> ConnectionProfile {
        let mut p = sample_profile(id);
        p.kind = ConnectionKind::Patroni;
        p.hosts.clear();
        p.patroni = Some(PatroniSettings {
            urls: vec!["https://pg-1:8008".into()],
            username: Some("patroni".into()),
            tls: Some(ApiTls {
                ca_path: Some(abs("api-ca.pem")),
                verify_hostname: true,
                client_cert: Some(ClientCert {
                    format: CertFormat::Pkcs12,
                    certificate_path: abs("api.p12"),
                    key_path: None,
                }),
            }),
            host_overrides: vec![],
        });
        p
    }

    /// ИФТ/{patroni-1}, ПСИ; подключения: a в ИФТ, b в patroni-1, c в ПСИ, d в корне.
    fn tree(s: &ConnectionStore) -> (String, String, String) {
        let ift = s.create_folder("ИФТ", None).unwrap();
        let k1 = s.create_folder("patroni-1", Some(&ift.id)).unwrap();
        let psi = s.create_folder("ПСИ", None).unwrap();
        s.upsert(in_folder("a", Some(&ift.id))).unwrap();
        s.upsert(in_folder("b", Some(&k1.id))).unwrap();
        s.upsert(in_folder("c", Some(&psi.id))).unwrap();
        s.upsert(in_folder("d", None)).unwrap();
        (ift.id, k1.id, psi.id)
    }

    fn ids<T>(items: &[T], f: impl Fn(&T) -> &str) -> Vec<String> {
        let mut v: Vec<String> = items.iter().map(|i| f(i).to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn exports_whole_tree() {
        let (_d, s) = store();
        let (ift, k1, psi) = tree(&s);
        let out = build_export(
            &s,
            &MemoryStore::default(),
            None,
            "9.9.9",
            "2026-10-17T12:00:00Z",
        )
        .unwrap();
        assert_eq!(out.format, "glafira-postgres-connections");
        assert_eq!(out.version, 1);
        assert_eq!(out.app_version, "9.9.9");
        assert_eq!(out.exported_at, "2026-10-17T12:00:00Z");
        let mut want = vec![ift, k1, psi];
        want.sort();
        assert_eq!(ids(&out.folders, |f| &f.id), want);
        assert_eq!(
            ids(&out.connections, |c| &c.profile.id),
            ["a", "b", "c", "d"]
        );

        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["format"], "glafira-postgres-connections");
        assert_eq!(v["version"], 1);
        assert!(v["exportedAt"].is_string());
        assert!(v["appVersion"].is_string());
        let np = &v["connections"][0]["needsPassword"];
        for flag in ["pg", "pgKey", "patroni", "patroniKey"] {
            assert!(np[flag].is_boolean(), "{flag}: {np}");
        }
        assert!(v["connections"][0]["hosts"].is_array());
    }

    #[test]
    fn exports_subtree_with_root_detached() {
        let (_d, s) = store();
        let (ift, k1, _psi) = tree(&s);
        let out = build_export(&s, &MemoryStore::default(), Some(&ift), "1", "now").unwrap();
        let mut want = vec![ift.clone(), k1.clone()];
        want.sort();
        assert_eq!(ids(&out.folders, |f| &f.id), want);
        let root = out.folders.iter().find(|f| f.id == ift).unwrap();
        assert_eq!(root.parent_id, None);
        let child = out.folders.iter().find(|f| f.id == k1).unwrap();
        assert_eq!(child.parent_id.as_deref(), Some(ift.as_str()));
        assert_eq!(ids(&out.connections, |c| &c.profile.id), ["a", "b"]);
        let v = serde_json::to_value(&out).unwrap();
        let root_json = v["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == ift.as_str())
            .unwrap();
        assert!(root_json["parentId"].is_null());
    }

    #[test]
    fn secrets_are_never_exported() {
        let (_d, s) = store();
        s.upsert(sample_profile("x")).unwrap();
        s.upsert(sample_profile("y")).unwrap();
        let secrets = MemoryStore::default();
        secrets
            .set(&secret_key("x", PG_PASSWORD), "s3cr3t")
            .unwrap();
        // У сервера без Patroni и без сертификата устаревшие пароли в keychain флагов не поднимают.
        for field in [PATRONI_PASSWORD, PATRONI_KEY_PASSWORD, PG_KEY_PASSWORD] {
            secrets.set(&secret_key("y", field), "s3cr3t").unwrap();
        }
        let out = build_export(&s, &secrets, None, "1", "now").unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("s3cr3t"));
        let x = out
            .connections
            .iter()
            .find(|c| c.profile.id == "x")
            .unwrap();
        assert_eq!(
            x.needs_password,
            NeedsPassword {
                pg: true,
                ..Default::default()
            }
        );
        let y = out
            .connections
            .iter()
            .find(|c| c.profile.id == "y")
            .unwrap();
        // PG-пароль профилю нужен всегда, но у `y` он не сохранён.
        assert_eq!(y.needs_password, NeedsPassword::default());
        assert!(!y.needs_password.any());
    }

    #[test]
    fn unknown_local_fields_are_not_exported() {
        let (_d, s) = store();
        s.upsert(sample_profile("a")).unwrap();
        s.update(|data| {
            data.connections[0]
                .extra
                .insert("futureField".into(), serde_json::json!("local"));
            Ok(())
        })
        .unwrap();
        let out = build_export(&s, &MemoryStore::default(), None, "1", "now").unwrap();
        assert!(out.connections[0].profile.extra.is_empty());
        assert!(!serde_json::to_string(&out).unwrap().contains("futureField"));
    }

    #[test]
    fn credentials_in_patroni_urls_are_not_exported() {
        let (_d, s) = store();
        let mut p = patroni_profile("p");
        let pt = p.patroni.as_mut().unwrap();
        pt.urls = vec![
            "https://admin:s3cr3t@pg-1:8008".into(),
            "http://pg-2:8008/health?user=a@b".into(),
        ];
        pt.host_overrides = vec![HostOverride {
            member: "pg-1".into(),
            host: None,
            port: None,
            api_url: Some("https://admin:s3cr3t@10.0.0.1:8008".into()),
        }];
        // Профили с диска не валидируются: адрес с паролем мог попасть в файл раньше.
        s.update(|data| {
            data.connections.push(p);
            Ok(())
        })
        .unwrap();
        let out = build_export(&s, &MemoryStore::default(), None, "1", "now").unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert!(
            !json.contains("s3cr3t") && !json.contains("admin:"),
            "{json}"
        );
        let pt = out.connections[0].profile.patroni.as_ref().unwrap();
        assert_eq!(
            pt.urls,
            ["https://pg-1:8008", "http://pg-2:8008/health?user=a@b"]
        );
        assert_eq!(
            pt.host_overrides[0].api_url.as_deref(),
            Some("https://10.0.0.1:8008")
        );
        // Исходный профиль в хранилище не меняется.
        assert!(
            s.list()[0].patroni.as_ref().unwrap().urls[0].contains("s3cr3t"),
            "экспорт не правит хранилище"
        );
    }

    #[test]
    fn url_credentials_helpers() {
        for (url, clean) in [
            ("https://pg-1:8008", "https://pg-1:8008"),
            ("https://u:p@pg-1:8008", "https://pg-1:8008"),
            ("https://u@pg-1:8008/x", "https://pg-1:8008/x"),
            ("https://u:p@w@pg-1/x", "https://pg-1/x"),
            ("http://pg-1/a@b", "http://pg-1/a@b"),
            ("http://pg-1?x=a@b", "http://pg-1?x=a@b"),
            ("no-scheme@host", "no-scheme@host"),
        ] {
            assert_eq!(strip_url_credentials(url), clean, "{url}");
            assert_eq!(has_url_credentials(url), url != clean, "{url}");
        }
    }

    #[test]
    fn needs_password_follows_used_secrets() {
        let secrets = MemoryStore::default();
        let mut p = sample_profile("a");
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        secrets
            .set(&secret_key("a", PATRONI_PASSWORD), "stale")
            .unwrap();
        let np = needs_password(&secrets, &p).unwrap();
        assert!(
            np.pg && !np.patroni,
            "секрет Patroni у сервера не используется"
        );
        p.pg.tls.client_cert = Some(ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: "/c.p12".into(),
            key_path: None,
        });
        secrets.set(&secret_key("a", PG_KEY_PASSWORD), "k").unwrap();
        assert!(needs_password(&secrets, &p).unwrap().pg_key);
    }

    #[test]
    fn patroni_secrets_are_flagged_and_empty_ones_are_not() {
        let secrets = MemoryStore::default();
        let p = patroni_profile("p");
        secrets
            .set(&secret_key("p", PATRONI_PASSWORD), "pw")
            .unwrap();
        secrets
            .set(&secret_key("p", PATRONI_KEY_PASSWORD), "k")
            .unwrap();
        // Пустое значение в keychain — «пароля нет».
        secrets.set(&secret_key("p", PG_PASSWORD), "").unwrap();
        assert_eq!(
            needs_password(&secrets, &p).unwrap(),
            NeedsPassword {
                patroni: true,
                patroni_key: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn leftover_key_passwords_without_client_certificates_are_not_flagged() {
        let (_d, s) = store();
        let mut p = sample_profile("t");
        p.pg.tls.ca_path = Some(abs("ca.pem"));
        s.upsert(p).unwrap();
        let secrets = MemoryStore::default();
        for f in [PG_KEY_PASSWORD, PATRONI_PASSWORD, PATRONI_KEY_PASSWORD] {
            secrets.set(&secret_key("t", f), "old").unwrap();
        }
        let out = build_export(&s, &secrets, None, "1", "now").unwrap();
        assert_eq!(out.connections[0].needs_password, NeedsPassword::default());
    }

    #[test]
    fn unknown_root_is_an_error() {
        let (_d, s) = store();
        tree(&s);
        let err = build_export(&s, &MemoryStore::default(), Some("nope"), "1", "now")
            .err()
            .unwrap();
        assert_eq!(err.key, "errors.folders.notFound");
    }

    // ---- импорт ----

    fn conn_json(id: &str, name: &str, folder: Option<&str>) -> serde_json::Value {
        let mut p = sample_profile(id);
        p.name = name.into();
        p.folder_id = folder.map(String::from);
        let mut v = serde_json::to_value(&p).unwrap();
        v["needsPassword"] =
            serde_json::json!({"pg": false, "pgKey": false, "patroni": false, "patroniKey": false});
        v
    }

    fn folder_json(id: &str, name: &str, parent: Option<&str>, order: i64) -> serde_json::Value {
        serde_json::json!({"id": id, "name": name, "parentId": parent, "order": order})
    }

    fn file(folders: Vec<serde_json::Value>, conns: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": "glafira-postgres-connections",
            "version": 1,
            "exportedAt": "now",
            "appVersion": "1",
            "folders": folders,
            "connections": conns,
        }))
        .unwrap()
    }

    fn local_folder(id: &str, name: &str, parent: Option<&str>) -> ConnectionFolder {
        ConnectionFolder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(String::from),
            order: 0,
            extra: Default::default(),
        }
    }

    fn opts(target: Option<&str>) -> ImportOptions<'_> {
        ImportOptions {
            target,
            update_existing: true,
            is_pro: true,
        }
    }

    fn all_exist(_: &str) -> bool {
        true
    }

    fn plan(data: &StoreData, bytes: &[u8], o: &ImportOptions) -> ImportPlan {
        let parsed = parse_exchange(bytes).unwrap();
        plan_import(data, &parsed, o, &all_exist).unwrap()
    }

    fn conn<'a>(data: &'a StoreData, id: &str) -> &'a ConnectionProfile {
        data.connections.iter().find(|c| c.id == id).unwrap()
    }

    fn folder<'a>(data: &'a StoreData, id: &str) -> &'a ConnectionFolder {
        data.folders.iter().find(|f| f.id == id).unwrap()
    }

    fn preview_ids(items: &[PreviewItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn kafka_exchange_file_is_rejected() {
        let bytes =
            br#"{"format":"glafira-connections","version":1,"folders":[],"connections":[]}"#;
        let err = parse_exchange(bytes).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(err.key, "errors.import.notGlafiraFile");
    }

    #[test]
    fn new_folders_and_connections_go_under_target_keeping_structure() {
        let data = StoreData {
            folders: vec![local_folder("t", "Target", None)],
            ..Default::default()
        };
        let bytes = file(
            vec![
                folder_json("k1", "patroni-1", Some("ift"), 0),
                folder_json("ift", "ИФТ", None, 0),
            ],
            vec![
                conn_json("a", "A", Some("ift")),
                conn_json("b", "B", Some("k1")),
                conn_json("c", "C", None),
            ],
        );
        let p = plan(&data, &bytes, &opts(Some("t")));
        let next = &p.next;
        assert_eq!(folder(next, "ift").parent_id.as_deref(), Some("t"));
        assert_eq!(folder(next, "ift").name, "ИФТ");
        assert_eq!(folder(next, "k1").parent_id.as_deref(), Some("ift"));
        assert_eq!(conn(next, "a").folder_id.as_deref(), Some("ift"));
        assert_eq!(conn(next, "b").folder_id.as_deref(), Some("k1"));
        assert_eq!(conn(next, "c").folder_id.as_deref(), Some("t"));
        assert_eq!(preview_ids(&p.preview.added), ["ift", "k1", "a", "b", "c"]);
        let b = p.preview.added.iter().find(|i| i.id == "b").unwrap();
        assert_eq!(b.kind, TreeItemKind::Connection);
        assert_eq!(b.path, ["Target", "ИФТ", "patroni-1"]);
        let k1 = p.preview.added.iter().find(|i| i.id == "k1").unwrap();
        assert_eq!(k1.kind, TreeItemKind::Folder);
        assert_eq!(k1.path, ["Target", "ИФТ"]);
        assert!(p.preview.updated.is_empty() && p.preview.unchanged.is_empty());
        check_tree(next).unwrap();
    }

    #[test]
    fn unknown_target_is_an_error() {
        let parsed = parse_exchange(&file(vec![], vec![])).unwrap();
        let err = plan_import(
            &StoreData::default(),
            &parsed,
            &opts(Some("nope")),
            &all_exist,
        )
        .err()
        .unwrap();
        assert_eq!(err.key, "errors.folders.notFound");
    }

    #[test]
    fn matched_connection_is_updated_only_with_update_existing() {
        let mut local = sample_profile("a");
        local.folder_id = Some("f".into());
        local.order = 7;
        local.extra.insert("pinned".into(), serde_json::json!(true));
        let data = StoreData {
            folders: vec![local_folder("f", "F", None)],
            connections: vec![local.clone()],
            ..Default::default()
        };
        let mut changed = conn_json("a", "conn-a", None);
        changed["hosts"] = serde_json::json!([{"host": "other", "port": 5432}]);
        let bytes = file(vec![], vec![changed]);

        let p = plan(&data, &bytes, &opts(None));
        let a = conn(&p.next, "a");
        assert_eq!(a.hosts[0].host, "other");
        assert_eq!(a.folder_id.as_deref(), Some("f"));
        assert_eq!(a.order, 7);
        assert_eq!(a.extra.get("pinned"), Some(&serde_json::json!(true)));
        assert_eq!(preview_ids(&p.preview.updated), ["a"]);
        assert_eq!(p.preview.updated[0].path, ["F"]);

        let p = plan(
            &data,
            &bytes,
            &ImportOptions {
                update_existing: false,
                ..opts(None)
            },
        );
        assert_eq!(conn(&p.next, "a"), &local);
        assert_eq!(preview_ids(&p.preview.unchanged), ["a"]);
        assert!(p.preview.updated.is_empty());

        // Те же настройки — «без изменений» и при update_existing.
        let same = file(vec![], vec![conn_json("a", "conn-a", None)]);
        let p = plan(&data, &same, &opts(None));
        assert_eq!(preview_ids(&p.preview.unchanged), ["a"]);
        assert_eq!(conn(&p.next, "a"), &local);
    }

    #[test]
    fn matched_folder_is_not_renamed_or_moved_and_receives_new_connections() {
        let data = StoreData {
            folders: vec![
                local_folder("p", "Parent", None),
                local_folder("f", "Local name", Some("p")),
            ],
            ..Default::default()
        };
        let bytes = file(
            vec![folder_json("f", "Renamed", None, 0)],
            vec![conn_json("n", "New", Some("f"))],
        );
        let p = plan(&data, &bytes, &opts(None));
        let f = folder(&p.next, "f");
        assert_eq!(f.name, "Local name");
        assert_eq!(f.parent_id.as_deref(), Some("p"));
        assert_eq!(conn(&p.next, "n").folder_id.as_deref(), Some("f"));
        assert_eq!(preview_ids(&p.preview.unchanged), ["f"]);
        assert_eq!(preview_ids(&p.preview.added), ["n"]);
        assert_eq!(p.preview.added[0].path, ["Parent", "Local name"]);
    }

    #[test]
    fn name_conflicts_get_suffixes() {
        let mut x = sample_profile("x");
        x.name = "Postgres".into();
        let mut y = sample_profile("y");
        y.name = "postgres (2)".into();
        let data = StoreData {
            folders: vec![local_folder("l1", "ИФТ", None)],
            connections: vec![x, y],
            ..Default::default()
        };
        let bytes = file(
            vec![folder_json("n1", "ифт", None, 0)],
            vec![conn_json("n", "Postgres", None)],
        );
        let p = plan(&data, &bytes, &opts(None));
        assert_eq!(folder(&p.next, "n1").name, "ифт (2)");
        assert_eq!(conn(&p.next, "n").name, "Postgres (3)");
        assert_eq!(p.preview.added[1].name, "Postgres (3)");
    }

    #[test]
    fn foreign_local_connections_are_untouched() {
        let mut mine = sample_profile("mine");
        mine.order = 3;
        let data = StoreData {
            connections: vec![mine.clone()],
            ..Default::default()
        };
        let p = plan(
            &data,
            &file(vec![], vec![conn_json("a", "A", None)]),
            &opts(None),
        );
        assert_eq!(conn(&p.next, "mine"), &mine);
        assert_eq!(conn(&p.next, "a").order, 4);
    }

    #[test]
    fn prod_becomes_read_only_in_free() {
        let mut prod = conn_json("p", "P", None);
        prod["environment"] = "prod".into();
        let bytes = file(vec![], vec![prod, conn_json("d", "D", None)]);
        let free = ImportOptions {
            is_pro: false,
            ..opts(None)
        };
        let p = plan(&StoreData::default(), &bytes, &free);
        assert!(conn(&p.next, "p").read_only);
        assert!(!conn(&p.next, "d").read_only);
        assert_eq!(p.preview.warnings.len(), 1);
        assert_eq!(p.preview.warnings[0].key, "warnings.import.prodReadOnly");
        assert_eq!(p.preview.warnings[0].params["count"], "1");

        let p = plan(&StoreData::default(), &bytes, &opts(None));
        assert!(!conn(&p.next, "p").read_only);
        assert!(p.preview.warnings.is_empty());

        // Pro: readOnly из файла сохраняется как есть.
        let mut ro = conn_json("r", "R", None);
        ro["environment"] = "prod".into();
        ro["readOnly"] = true.into();
        let p = plan(&StoreData::default(), &file(vec![], vec![ro]), &opts(None));
        assert!(conn(&p.next, "r").read_only);
    }

    #[test]
    fn prod_update_in_free_is_forced_read_only() {
        let mut local = sample_profile("p");
        local.environment = Environment::Prod;
        local.read_only = false;
        let data = StoreData {
            connections: vec![local],
            ..Default::default()
        };
        let mut incoming = conn_json("p", "renamed", None);
        incoming["environment"] = "prod".into();
        let free = ImportOptions {
            is_pro: false,
            ..opts(None)
        };
        let p = plan(&data, &file(vec![], vec![incoming]), &free);
        assert_eq!(preview_ids(&p.preview.updated), ["p"]);
        assert!(conn(&p.next, "p").read_only);
        assert_eq!(p.preview.warnings[0].key, "warnings.import.prodReadOnly");
    }

    #[test]
    fn identical_prod_in_free_is_unchanged_and_keeps_read_only() {
        let mut local = sample_profile("p");
        local.environment = Environment::Prod;
        local.read_only = false;
        let data = StoreData {
            connections: vec![local.clone()],
            ..Default::default()
        };
        let mut same = conn_json("p", "conn-p", None);
        same["environment"] = "prod".into();
        let free = ImportOptions {
            is_pro: false,
            ..opts(None)
        };
        let p = plan(&data, &file(vec![], vec![same]), &free);
        assert_eq!(preview_ids(&p.preview.unchanged), ["p"]);
        assert!(p.preview.updated.is_empty());
        assert!(p.preview.warnings.is_empty());
        assert_eq!(conn(&p.next, "p"), &local);
    }

    #[test]
    fn duplicate_connection_ids_are_skipped_with_reason() {
        let bytes = file(
            vec![],
            vec![
                conn_json("a", "First", None),
                conn_json("a", "Second", None),
            ],
        );
        let parsed = parse_exchange(&bytes).unwrap();
        assert_eq!(parsed.connections.len(), 1);
        assert_eq!(parsed.connections[0].0.name, "First");
        assert_eq!(parsed.skipped.len(), 1);
        assert_eq!(parsed.skipped[0].name, "Second");
        assert_eq!(parsed.skipped[0].reason.key, "errors.import.duplicateId");
    }

    #[test]
    fn credentials_in_patroni_urls_are_skipped() {
        let mut leaky = serde_json::to_value(patroni_profile("s")).unwrap();
        leaky["name"] = "Leaky".into();
        leaky["patroni"]["urls"] = serde_json::json!(["https://admin:s3cr3t@pg-1:8008"]);
        let mut leaky_override = serde_json::to_value(patroni_profile("o")).unwrap();
        leaky_override["name"] = "LeakyOverride".into();
        leaky_override["patroni"]["hostOverrides"] =
            serde_json::json!([{"member": "pg-1", "apiUrl": "http://u:p@10.0.0.1:8008"}]);
        let ok = serde_json::to_value(patroni_profile("ok")).unwrap();
        let bytes = file(vec![], vec![leaky, leaky_override, ok]);
        let parsed = parse_exchange(&bytes).unwrap();
        assert_eq!(parsed.connections.len(), 1);
        assert_eq!(parsed.connections[0].0.id, "ok");
        assert_eq!(
            parsed
                .skipped
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Leaky", "LeakyOverride"]
        );
        assert!(
            parsed
                .skipped
                .iter()
                .all(|s| s.reason.key == "errors.import.secretInUrl")
        );
        let p = plan_import(&StoreData::default(), &parsed, &opts(None), &all_exist).unwrap();
        assert!(
            !serde_json::to_string(&p.next.connections)
                .unwrap()
                .contains("s3cr3t")
        );
    }

    #[test]
    fn imported_profiles_are_normalized_like_saved_ones() {
        let mut padded = conn_json("a", "  Padded  ", None);
        padded["hosts"] = serde_json::json!([{"host": " db1 ", "port": 5432}]);
        padded["pg"]["username"] = " u ".into();
        let mut patroni = serde_json::to_value(patroni_profile("p")).unwrap();
        patroni["hosts"] = serde_json::json!([{"host": "ignored", "port": 5432}]);
        patroni["patroni"]["urls"] = serde_json::json!([" https://pg-1:8008/ "]);
        let parsed = parse_exchange(&file(vec![], vec![padded, patroni])).unwrap();
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        let a = &parsed.connections[0].0;
        assert_eq!(a.name, "Padded");
        assert_eq!(a.hosts[0].host, "db1");
        assert_eq!(a.pg.username, "u");
        let p = &parsed.connections[1].0;
        assert!(p.hosts.is_empty(), "у Patroni узлы приходят из /cluster");
        assert_eq!(p.patroni.as_ref().unwrap().urls, ["https://pg-1:8008"]);
    }

    #[test]
    fn missing_certificate_files_are_warned_about() {
        let mut tls = conn_json("t", "T", None);
        tls["pg"]["tls"] = serde_json::json!({
            "caPath": abs("ca.pem"),
            "clientCert": {"format": "pem", "certificatePath": abs("c.pem"), "keyPath": abs("k.pem")}
        });
        let mut tls2 = tls.clone();
        tls2["id"] = "t2".into();
        let bytes = file(vec![], vec![tls, tls2, conn_json("plain", "Plain", None)]);
        let parsed = parse_exchange(&bytes).unwrap();
        let missing = |p: &str| !p.ends_with("k.pem");
        let p = plan_import(&StoreData::default(), &parsed, &opts(None), &missing).unwrap();
        assert_eq!(p.preview.warnings.len(), 1);
        assert_eq!(p.preview.warnings[0].key, "warnings.import.missingFiles");
        assert_eq!(p.preview.warnings[0].params["count"], "2");
        let p = plan_import(&StoreData::default(), &parsed, &opts(None), &all_exist).unwrap();
        assert!(p.preview.warnings.is_empty());

        // Обновлённое подключение тоже проверяется.
        let data = StoreData {
            connections: vec![sample_profile("t")],
            ..Default::default()
        };
        let only_t = file(
            vec![],
            vec![serde_json::to_value(&parsed.connections[0].0).unwrap()],
        );
        let parsed = parse_exchange(&only_t).unwrap();
        let p = plan_import(&data, &parsed, &opts(None), &missing).unwrap();
        assert_eq!(preview_ids(&p.preview.updated), ["t"]);
        assert_eq!(p.preview.warnings.len(), 1);
        assert_eq!(p.preview.warnings[0].key, "warnings.import.missingFiles");
        assert_eq!(p.preview.warnings[0].params["count"], "1");
    }

    #[test]
    fn certificate_files_of_the_patroni_api_are_checked_too() {
        let p = patroni_profile("p");
        assert!(!misses_files(&p, &all_exist));
        assert!(misses_files(&p, &|path: &str| !path.ends_with("api-ca.pem")));
        assert!(misses_files(&p, &|path: &str| !path.ends_with("api.p12")));
        let mut pem = p.clone();
        pem.patroni
            .as_mut()
            .unwrap()
            .tls
            .as_mut()
            .unwrap()
            .client_cert = Some(ClientCert {
            format: CertFormat::Pem,
            certificate_path: abs("api.pem"),
            key_path: Some(abs("api.key")),
        });
        assert!(misses_files(&pem, &|path: &str| !path.ends_with("api.key")));
        // Пустой путь и подключение без TLS файлов не требуют.
        let mut blank = sample_profile("b");
        blank.pg.tls.ca_path = Some("  ".into());
        assert!(!misses_files(&blank, &|_: &str| false));
        assert!(!misses_files(&sample_profile("s"), &|_: &str| false));
    }

    #[test]
    fn partial_needs_password_flags_default_to_false() {
        let mut a = conn_json("a", "A", None);
        a["needsPassword"] = serde_json::json!({"patroni": true});
        let mut b = conn_json("b", "B", None);
        b.as_object_mut().unwrap().remove("needsPassword");
        let parsed = parse_exchange(&file(vec![], vec![a, b])).unwrap();
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(
            parsed.connections[0].1,
            NeedsPassword {
                patroni: true,
                ..Default::default()
            }
        );
        assert_eq!(parsed.connections[1].1, NeedsPassword::default());
    }

    #[test]
    fn needs_passwords_counts_new_connections_only() {
        let mut a = conn_json("a", "A", None);
        a["needsPassword"]["pg"] = true.into();
        let mut b = conn_json("b", "B", None);
        b["needsPassword"]["pgKey"] = true.into();
        b["hosts"] = serde_json::json!([{"host": "changed", "port": 1}]);
        let mut c = conn_json("c", "C", None);
        c["needsPassword"]["patroniKey"] = true.into();
        let d = conn_json("d", "D", None);
        let data = StoreData {
            connections: vec![sample_profile("b")],
            ..Default::default()
        };
        let p = plan(&data, &file(vec![], vec![a, b, c, d]), &opts(None));
        assert_eq!(preview_ids(&p.preview.updated), ["b"]);
        assert_eq!(p.preview.needs_passwords, 2);
    }

    #[test]
    fn parse_exchange_tolerates_unknown_fields_and_skips_unsupported() {
        let mut ok = conn_json("ok", "OK", None);
        ok["futureField"] = serde_json::json!({"x": 1});
        let mut future = conn_json("future", "Future", None);
        future["kind"] = "multiMaster".into();
        let mut invalid = conn_json("bad", "Bad", None);
        invalid["hosts"] = serde_json::json!([{"host": "a b", "port": 5432}]);
        let mut v: serde_json::Value =
            serde_json::from_slice(&file(vec![], vec![ok, future, invalid])).unwrap();
        v["somethingNew"] = serde_json::json!([1, 2]);
        let parsed = parse_exchange(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(parsed.connections.len(), 1);
        assert_eq!(parsed.connections[0].0.id, "ok");
        assert!(parsed.connections[0].0.extra.is_empty());
        assert_eq!(parsed.skipped.len(), 2);
        assert_eq!(parsed.skipped[0].name, "Future");
        assert_eq!(
            parsed.skipped[0].reason.key,
            "errors.import.unsupportedConnection"
        );
        assert_eq!(parsed.skipped[1].name, "Bad");
        assert_eq!(
            parsed.skipped[1].reason.key,
            "errors.connections.hostInvalid"
        );

        let p = plan_import(&StoreData::default(), &parsed, &opts(None), &all_exist).unwrap();
        assert_eq!(preview_ids(&p.preview.added), ["ok"]);
        assert_eq!(p.preview.skipped.len(), 2);

        v["version"] = 2.into();
        let err = parse_exchange(&serde_json::to_vec(&v).unwrap())
            .err()
            .unwrap();
        assert_eq!(err.key, "errors.import.incompatibleVersion");
        v["version"] = 1.into();
        v["format"] = "other".into();
        let err = parse_exchange(&serde_json::to_vec(&v).unwrap())
            .err()
            .unwrap();
        assert_eq!(err.key, "errors.import.notGlafiraFile");
        let err = parse_exchange(b"{not json").err().unwrap();
        assert_eq!(err.key, "errors.import.invalidJson");
        let err = parse_exchange(b"[1, 2]").err().unwrap();
        assert_eq!(err.key, "errors.import.notGlafiraFile");
    }

    #[test]
    fn export_then_import_into_empty_store_round_trips() {
        let (_d, s) = store();
        let (_ift, _k1, psi) = tree(&s);
        let mut x = patroni_profile("x");
        x.folder_id = Some(psi.clone());
        x.environment = Environment::Stage;
        s.upsert(x).unwrap();
        let secrets = MemoryStore::default();
        secrets
            .set(&secret_key("x", PATRONI_PASSWORD), "s3cr3t")
            .unwrap();
        let out = build_export(&s, &secrets, None, "1", "now").unwrap();
        let bytes = serde_json::to_vec(&out).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("s3cr3t"));

        let parsed = parse_exchange(&bytes).unwrap();
        assert!(parsed.skipped.is_empty());
        let p = plan_import(&StoreData::default(), &parsed, &opts(None), &all_exist).unwrap();
        assert_eq!(p.preview.needs_passwords, 1);
        let sort_f = |mut v: Vec<ConnectionFolder>| {
            v.sort_by(|a, b| a.id.cmp(&b.id));
            v
        };
        let sort_c = |mut v: Vec<ConnectionProfile>| {
            v.sort_by(|a, b| a.id.cmp(&b.id));
            v
        };
        assert_eq!(sort_f(p.next.folders.clone()), sort_f(s.list_folders()));
        assert_eq!(sort_c(p.next.connections.clone()), sort_c(s.list()));
    }
}
