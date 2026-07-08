use nanostores::{map, NanoMap};
use serde::{Deserialize, Serialize};

#[derive(NanoMap, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    name: String,
    #[serde(rename = "display")]
    display_name: Option<String>,
}

// Unrelated serde attributes (values, nested lists, bare flags) must not
// break the derive, and every serde rename_all casing must be honored.
#[derive(NanoMap, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_count: Option<u32>,
    base_url: String,
}

#[derive(NanoMap, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct Env {
    log_level: String,
}

fn main() {
    let store = map(User {
        name: "Ada".to_owned(),
        display_name: None,
    });

    store.set_name("Grace".to_owned());
    store
        .set_key("display", serde_json::json!("G.H."))
        .unwrap();

    assert_eq!(store.get().name, "Grace");
    assert_eq!(User::KEYS, &["name", "display"]);
    assert_eq!(Config::KEYS, &["retry-count", "base-url"]);
    assert_eq!(Env::KEYS, &["LOG_LEVEL"]);
}
