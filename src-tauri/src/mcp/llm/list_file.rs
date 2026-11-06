//! Общий каркас файлов `llm-requests.json` и `llm-proposals.json`: `{ "schemaVersion": 1, "<key>": [ … ] }`, 0600,
//! атомарная запись. Хранилища запросов и предложений различаются только элементом и ключом списка.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::storage::fs::{newer_schema_error, read_json_or_quarantine, write_json};
use crate::sync::MutexExt;

const SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileBody<T> {
    #[serde(default)]
    schema_version: u32,
    #[serde(flatten)]
    lists: BTreeMap<String, Vec<T>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileBodyRef<'a, T> {
    schema_version: u32,
    #[serde(flatten)]
    lists: BTreeMap<&'static str, &'a [T]>,
}

/// Список элементов в файле под мьютексом: чтение из памяти, правка — копия → запись → замена.
pub(super) struct ListFile<T> {
    path: PathBuf,
    key: &'static str,
    items: Mutex<Vec<T>>,
    /// Файл записан более новой версией или не читается: запись запрещена, чтобы его не затереть.
    blocked: Option<AppError>,
}

impl<T: Clone + Serialize + DeserializeOwned> ListFile<T> {
    pub(super) fn open(dir: &Path, file: &str, key: &'static str) -> Self {
        let path = dir.join(file);
        let (items, blocked) = match read_json_or_quarantine::<FileBody<T>>(&path, SCHEMA_VERSION) {
            Ok(Some(f)) if f.schema_version > SCHEMA_VERSION => (
                Vec::new(),
                Some(newer_schema_error(&path, f.schema_version)),
            ),
            Ok(Some(mut f)) => (f.lists.remove(key).unwrap_or_default(), None),
            Ok(None) => (Vec::new(), None),
            Err(e) => {
                tracing::warn!("{} is not used: {}", path.display(), e.message);
                (Vec::new(), Some(e))
            }
        };
        Self {
            path,
            key,
            items: Mutex::new(items),
            blocked,
        }
    }

    /// Правка под блокировкой. Если запись не удалась, в памяти остаётся прежнее состояние.
    pub(super) fn update<R>(&self, f: impl FnOnce(&mut Vec<T>) -> AppResult<R>) -> AppResult<R> {
        if let Some(e) = &self.blocked {
            return Err(e.clone());
        }
        let mut items = self.items.lock_safe();
        let mut next = items.clone();
        let out = f(&mut next)?;
        write_json(
            &self.path,
            &FileBodyRef {
                schema_version: SCHEMA_VERSION,
                lists: BTreeMap::from([(self.key, next.as_slice())]),
            },
        )?;
        *items = next;
        Ok(out)
    }

    /// Чтение из памяти; блокировка держится только на время `f`.
    pub(super) fn read<R>(&self, f: impl FnOnce(&[T]) -> R) -> R {
        f(&self.items.lock_safe())
    }
}
