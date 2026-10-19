//! Операции над деревом папок подключений. Каждая операция — одна транзакция
//! `ConnectionStore::update`: инварианты (ссылки, циклы, имена) проверяет `check_tree`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{AppResult, ErrorCode};
use crate::storage::connections::{
    ConnectionFolder, ConnectionStore, StoreData, connection_not_found, folder_not_found,
    validate_folder_name,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeItemKind {
    Folder,
    Connection,
}

/// Ссылка на элемент дерева: папку или подключение.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeItemRef {
    pub kind: TreeItemKind,
    pub id: String,
}

impl TreeItemRef {
    fn exists(&self, data: &StoreData) -> AppResult<()> {
        let found = match self.kind {
            TreeItemKind::Folder => data.folders.iter().any(|f| f.id == self.id),
            TreeItemKind::Connection => data.connections.iter().any(|c| c.id == self.id),
        };
        if found {
            return Ok(());
        }
        Err(match self.kind {
            TreeItemKind::Folder => folder_not_found(&self.id),
            TreeItemKind::Connection => connection_not_found(&self.id),
        })
    }
}

/// Новый порядок одного уровня одного вида: `level` — id уровня по возрастанию `order`
/// (без перемещаемых), `moved` — перемещаемые id в порядке запроса.
fn arrange(mut level: Vec<String>, moved: Vec<String>, before: Option<&str>) -> Vec<String> {
    let at = before
        .and_then(|b| level.iter().position(|id| id == b))
        .unwrap_or(level.len());
    level.splice(at..at, moved);
    level
}

impl ConnectionStore {
    /// Создаёт папку в конце уровня `parent_id`.
    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
    ) -> AppResult<ConnectionFolder> {
        validate_folder_name(name)?;
        self.update(|data| {
            let parent_id = parent_id.map(str::to_string);
            let order = data
                .folders
                .iter()
                .filter(|f| f.parent_id == parent_id)
                .map(|f| f.order + 1)
                .max()
                .unwrap_or(0);
            let folder = ConnectionFolder {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.trim().to_string(),
                parent_id,
                order,
                extra: Default::default(),
            };
            data.folders.push(folder.clone());
            Ok(folder)
        })
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> AppResult<ConnectionFolder> {
        validate_folder_name(name)?;
        self.update(|data| {
            let folder = data
                .folders
                .iter_mut()
                .find(|f| f.id == id)
                .ok_or_else(|| folder_not_found(id))?;
            folder.name = name.trim().to_string();
            Ok(folder.clone())
        })
    }

    /// Удаляет только пустую папку: без вложенных папок и подключений.
    pub fn delete_folder(&self, id: &str) -> AppResult<()> {
        self.update(|data| {
            if !data.folders.iter().any(|f| f.id == id) {
                return Err(folder_not_found(id));
            }
            let busy = data
                .folders
                .iter()
                .any(|f| f.parent_id.as_deref() == Some(id))
                || data
                    .connections
                    .iter()
                    .any(|c| c.folder_id.as_deref() == Some(id));
            if busy {
                return Err(app_err!(
                    ErrorCode::InvalidInput,
                    "errors.folders.notEmpty",
                    "The folder is not empty: move or delete its connections and folders first"
                ));
            }
            data.folders.retain(|f| f.id != id);
            Ok(())
        })
    }

    /// Переносит элементы в `parent_id` и ставит их перед `before` (или в конец уровня).
    /// Папки и подключения уровня нумеруются раздельно, `order` = 0..n; `before` другого вида,
    /// не лежащий на уровне или сам перемещаемый, означает «в конец».
    pub fn move_items(
        &self,
        items: &[TreeItemRef],
        parent_id: Option<&str>,
        before: Option<&TreeItemRef>,
    ) -> AppResult<()> {
        self.update(|data| {
            let mut seen = HashSet::new();
            let items: Vec<&TreeItemRef> = items.iter().filter(|i| seen.insert(*i)).collect();
            for item in &items {
                item.exists(data)?;
            }
            let moved = |kind: TreeItemKind| -> Vec<String> {
                items
                    .iter()
                    .filter(|i| i.kind == kind)
                    .map(|i| i.id.clone())
                    .collect()
            };
            let before_of =
                |kind: TreeItemKind| before.filter(|b| b.kind == kind).map(|b| b.id.as_str());
            let target = parent_id.map(str::to_string);

            let moved_folders = moved(TreeItemKind::Folder);
            let mut level: Vec<&ConnectionFolder> = data
                .folders
                .iter()
                .filter(|f| f.parent_id == target && !moved_folders.contains(&f.id))
                .collect();
            level.sort_by_key(|f| f.order);
            let level = level.into_iter().map(|f| f.id.clone()).collect();
            let order = arrange(level, moved_folders, before_of(TreeItemKind::Folder));
            for f in &mut data.folders {
                if let Some(i) = order.iter().position(|id| *id == f.id) {
                    f.parent_id = target.clone();
                    f.order = i as i64;
                }
            }

            let moved_conns = moved(TreeItemKind::Connection);
            let mut level: Vec<_> = data
                .connections
                .iter()
                .filter(|c| c.folder_id == target && !moved_conns.contains(&c.id))
                .collect();
            level.sort_by_key(|c| c.order);
            let level = level.into_iter().map(|c| c.id.clone()).collect();
            let order = arrange(level, moved_conns, before_of(TreeItemKind::Connection));
            for c in &mut data.connections {
                if let Some(i) = order.iter().position(|id| *id == c.id) {
                    c.folder_id = target.clone();
                    c.order = i as i64;
                }
            }
            Ok(())
        })
    }

    /// Имена папок от корня до `id` включительно; пусто, если папки нет.
    pub fn folder_path(&self, id: &str) -> Vec<String> {
        let folders = self.list_folders();
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = Some(id);
        while let Some(fid) = cur {
            if !seen.insert(fid) {
                break;
            }
            let Some(f) = folders.iter().find(|f| f.id == fid) else {
                break;
            };
            path.push(f.name.clone());
            cur = f.parent_id.as_deref();
        }
        path.reverse();
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::storage::connections::ConnectionStore;

    fn store() -> (tempfile::TempDir, ConnectionStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ConnectionStore::open(d.path()).unwrap();
        (d, s)
    }

    #[test]
    fn create_rename_delete() {
        let (_d, s) = store();
        let a = s.create_folder("  ИФТ ", None).unwrap();
        assert_eq!(a.name, "ИФТ");
        let b = s.create_folder("kafka-1", Some(&a.id)).unwrap();
        assert_eq!(s.folder_path(&b.id), vec!["ИФТ", "kafka-1"]);
        assert_eq!(
            s.create_folder("ифт", None).unwrap_err().key,
            "errors.folders.nameTaken"
        );
        assert_eq!(
            s.delete_folder(&a.id).unwrap_err().key,
            "errors.folders.notEmpty"
        );
        s.rename_folder(&b.id, "kafka-2").unwrap();
        s.delete_folder(&b.id).unwrap();
        s.delete_folder(&a.id).unwrap();
        assert!(s.list_folders().is_empty());
    }

    #[test]
    fn move_into_descendant_is_a_cycle() {
        let (_d, s) = store();
        let a = s.create_folder("A", None).unwrap();
        let b = s.create_folder("B", Some(&a.id)).unwrap();
        let e = s
            .move_items(
                &[TreeItemRef {
                    kind: TreeItemKind::Folder,
                    id: a.id.clone(),
                }],
                Some(&b.id),
                None,
            )
            .unwrap_err();
        assert_eq!(e.key, "errors.folders.cycle");
        assert_eq!(
            s.list_folders()
                .iter()
                .find(|f| f.id == a.id)
                .unwrap()
                .parent_id,
            None,
            "ничего не изменилось"
        );
    }

    #[test]
    fn move_connections_and_reorder() {
        let (_d, s) = store();
        for id in ["a", "b", "c"] {
            s.upsert(sample_profile(id)).unwrap();
        }
        let f = s.create_folder("F", None).unwrap();
        let conn = |id: &str| TreeItemRef {
            kind: TreeItemKind::Connection,
            id: id.into(),
        };
        s.move_items(&[conn("c"), conn("a")], Some(&f.id), None)
            .unwrap();
        let in_f: Vec<_> = {
            let mut v: Vec<_> = s
                .list()
                .into_iter()
                .filter(|p| p.folder_id.as_deref() == Some(&f.id))
                .collect();
            v.sort_by_key(|p| p.order);
            v.into_iter().map(|p| p.id).collect()
        };
        assert_eq!(in_f, vec!["c", "a"]);
        s.move_items(&[conn("a")], Some(&f.id), Some(&conn("c")))
            .unwrap();
        let mut v: Vec<_> = s
            .list()
            .into_iter()
            .filter(|p| p.folder_id.as_deref() == Some(&f.id))
            .collect();
        v.sort_by_key(|p| p.order);
        assert_eq!(
            v.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
        assert_eq!(s.get("b").unwrap().folder_id, None);
    }
    fn folder_ref(id: &str) -> TreeItemRef {
        TreeItemRef {
            kind: TreeItemKind::Folder,
            id: id.into(),
        }
    }

    #[test]
    fn folders_are_reordered_and_conflicting_names_rejected() {
        let (_d, s) = store();
        let x = s.create_folder("X", None).unwrap();
        let y = s.create_folder("Y", None).unwrap();
        let p = s.create_folder("P", None).unwrap();
        assert_eq!((x.order, y.order, p.order), (0, 1, 2));
        s.move_items(&[folder_ref(&p.id)], None, Some(&folder_ref(&x.id)))
            .unwrap();
        let mut root = s.list_folders();
        root.sort_by_key(|f| f.order);
        assert_eq!(
            root.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["P", "X", "Y"]
        );
        let inner = s.create_folder("x", Some(&p.id)).unwrap();
        assert_eq!(
            s.move_items(&[folder_ref(&inner.id)], None, None)
                .unwrap_err()
                .key,
            "errors.folders.nameTaken"
        );
        assert_eq!(
            s.rename_folder(&y.id, "x").unwrap_err().key,
            "errors.folders.nameTaken"
        );
        assert_eq!(
            s.rename_folder(&y.id, "  ").unwrap_err().key,
            "errors.folders.nameEmpty"
        );
    }

    #[test]
    fn unknown_items_and_parents_are_not_found() {
        let (_d, s) = store();
        s.upsert(sample_profile("a")).unwrap();
        let conn = TreeItemRef {
            kind: TreeItemKind::Connection,
            id: "nope".into(),
        };
        assert_eq!(
            s.move_items(&[conn], None, None).unwrap_err().key,
            "errors.connections.notFound"
        );
        let a = TreeItemRef {
            kind: TreeItemKind::Connection,
            id: "a".into(),
        };
        assert_eq!(
            s.move_items(&[a], Some("nope"), None).unwrap_err().key,
            "errors.folders.notFound"
        );
        assert_eq!(
            s.create_folder("F", Some("nope")).unwrap_err().key,
            "errors.folders.notFound"
        );
        assert_eq!(
            s.delete_folder("nope").unwrap_err().key,
            "errors.folders.notFound"
        );
        assert!(s.folder_path("nope").is_empty());
    }

    #[test]
    fn item_refs_deserialize_from_frontend_shape() {
        let r: TreeItemRef =
            serde_json::from_value(serde_json::json!({"kind": "folder", "id": "f"})).unwrap();
        assert_eq!(r, folder_ref("f"));
    }
}
