//! Ключ модели в таблице цен по имени модели Ollama и обратно — имя облачной модели Ollama по ключу таблицы.

/// Ключ облачной модели Ollama в таблице цен — имя без тега `cloud`, в нижнем регистре:
/// - `glm-5.3-flash:cloud` → `glm-5.3-flash`;
/// - `gpt-oss:120b-cloud` → `gpt-oss:120b`.
///
/// Облачная модель Ollama — та, у которой тег `cloud` или оканчивается на `-cloud` (как определяет и Glafira for
/// PostgreSQL). У локальной модели (другой тег, без тега) ключа нет: её запуск ничего не стоит.
pub fn model_key(name: &str) -> Option<String> {
    let (base, tag) = name.trim().rsplit_once(':')?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }
    let tag = tag.trim().to_ascii_lowercase();
    let key = if tag == "cloud" {
        base.to_string()
    } else {
        let size = tag.strip_suffix("-cloud").filter(|s| !s.is_empty())?;
        format!("{base}:{size}")
    };
    Some(key.to_ascii_lowercase())
}

/// Имя облачной модели Ollama по ключу таблицы цен (обратное `model_key`): ключ без размера получает тег `cloud`, ключ с
/// размером — суффикс `-cloud` у тега:
/// - `glm-5.3-flash` → `glm-5.3-flash:cloud`;
/// - `gpt-oss:120b` → `gpt-oss:120b-cloud`.
pub fn cloud_model_name(key: &str) -> String {
    let key = key.trim();
    if key.contains(':') {
        format!("{key}-cloud")
    } else {
        format!("{key}:cloud")
    }
}
