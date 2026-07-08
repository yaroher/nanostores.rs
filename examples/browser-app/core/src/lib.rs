#![cfg(target_arch = "wasm32")]

use nanostores::{NanoMap, batched, computed};
use serde::{Deserialize, Serialize};
use tsify::Tsify;
use wasm_bindgen::prelude::*;

#[derive(NanoMap, Tsify, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub name: String,
    pub age: u32,
    pub display_name: Option<String>,
}

nanostores_wasm::define_stores! {
    pub fn stores() -> StoreHandles {
        atom count: i64 = 0;
        map user: User = User {
            name: "Ada".to_owned(),
            age: 36,
            display_name: Some("Ada Lovelace".to_owned()),
        };
        readable doubled: i64 = computed((count().clone(),), |value| value * 2);
        readable summary: String = batched((count().clone(), user().clone()), summary_text);
    }
}

#[wasm_bindgen]
pub fn increment() {
    count().set(count().get() + 1);
}

/// Async action: any awaited work (network fetch, timer) followed by store
/// writes. Projections in the UI update reactively when the write lands.
#[wasm_bindgen]
pub async fn load_user(name: String) -> Result<(), JsValue> {
    gloo_timers::future::TimeoutFuture::new(400).await; // simulated fetch
    user().set(User {
        display_name: Some(format!("{name} (loaded)")),
        name,
        age: user().get().age + 1,
    });
    Ok(())
}

fn summary_text(count: i64, user: User) -> String {
    let name = user.display_name.as_deref().unwrap_or(&user.name);
    format!("{name} has {count} clicks")
}
