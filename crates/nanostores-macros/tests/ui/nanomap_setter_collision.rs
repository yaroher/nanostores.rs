use nanostores::NanoMap;
use serde::{Deserialize, Serialize};

#[derive(NanoMap, Clone, PartialEq, Serialize, Deserialize)]
struct Bad {
    key: String,
}

fn main() {}
