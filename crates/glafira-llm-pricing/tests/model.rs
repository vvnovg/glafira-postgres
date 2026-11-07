//! Ключ модели в таблице цен по имени модели Ollama, имя облачной модели Ollama по ключу и поиск цен по ключу.

use glafira_llm_pricing::{builtin, cloud_model_name, model_key};

#[test]
fn the_key_is_the_name_without_the_cloud_tag() {
    for (name, key) in [
        ("glm-5.3-flash:cloud", "glm-5.3-flash"),
        ("deepseek-v4.1-flash:cloud", "deepseek-v4.1-flash"),
        ("gpt-oss:120b-cloud", "gpt-oss:120b"),
        ("gpt-oss:20b-cloud", "gpt-oss:20b"),
        (" GLM-5.3:Cloud ", "glm-5.3"),
        ("Kimi-K3:1T-CLOUD", "kimi-k3:1t"),
    ] {
        assert_eq!(model_key(name).as_deref(), Some(key), "{name}");
    }
}

#[test]
fn a_local_model_has_no_key() {
    for name in [
        "qwen3.6-64k:35b-a3b",
        "qwen3.8:27b",
        "gemma4",
        "gemma4:latest",
        "glm-5.3-flash",
        "llama3:cloudy",
        "llama3:cloud-x",
        ":cloud",
        "gpt-oss:-cloud",
        "",
        "   ",
    ] {
        assert_eq!(model_key(name), None, "{name:?}");
    }
}

#[test]
fn prices_are_found_by_the_exact_key_or_the_name_without_the_size() {
    let t = builtin();
    assert_eq!(t.find("gpt-oss:120b").unwrap().model, "gpt-oss:120b");
    assert_eq!(t.find("gpt-oss:20b").unwrap().model, "gpt-oss:20b");
    assert_eq!(t.find("glm-5.3-flash").unwrap().model, "glm-5.3-flash");
    // Страница называет модель без размера, а облачный тег Ollama — с размером.
    assert_eq!(t.find("gemma4:31b").unwrap().model, "gemma4");
    assert_eq!(t.find("GLM-5.3").unwrap().model, "glm-5.3");
    assert!(t.find("gpt-oss:7b").is_none(), "у gpt-oss цены по размерам");
    assert!(t.find("unknown-model").is_none());
}

#[test]
fn the_cloud_model_name_is_the_key_with_the_cloud_tag() {
    // Ключ без размера — тег `cloud`; с размером — суффикс `-cloud` у тега (так называет облачные модели Ollama).
    for (key, name) in [
        ("glm-5.3-flash", "glm-5.3-flash:cloud"),
        ("deepseek-v4.1-flash", "deepseek-v4.1-flash:cloud"),
        ("gpt-oss:120b", "gpt-oss:120b-cloud"),
        ("gpt-oss:20b", "gpt-oss:20b-cloud"),
    ] {
        assert_eq!(cloud_model_name(key), name, "{key}");
        assert_eq!(model_key(name).as_deref(), Some(key), "обратно: {name}");
    }
}

#[test]
fn every_model_of_the_table_has_a_cloud_name_in_the_page_order() {
    let t = builtin();
    let names: Vec<String> = t.cloud_models().map(|(name, _)| name).collect();
    assert_eq!(names.len(), t.models.len());
    assert_eq!(names[0], "beam:cloud");
    assert!(names.iter().any(|n| n == "gpt-oss:120b-cloud"), "{names:?}");
    for ((name, prices), m) in t.cloud_models().zip(&t.models) {
        assert_eq!(prices, m, "{name}");
        assert_eq!(model_key(&name).as_deref(), Some(m.model.as_str()));
        assert_eq!(t.find(&model_key(&name).unwrap()), Some(m), "{name}");
    }
}
