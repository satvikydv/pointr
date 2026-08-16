use std::fs;

fn main() {
    // Reads the project root's .env (same gitignored file the backend uses,
    // two levels up from this crate) for POINTR_ENV=local|prod, baked in at
    // compile time via env!() in api_config.rs. Defaults to "local" if the
    // file or key is missing, so a fresh clone never accidentally builds
    // pointed at someone else's prod backend/key without an explicit opt-in.
    let env_value = fs::read_to_string("../../.env")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("POINTR_ENV=")
                    .map(|v| v.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "local".to_string());

    let (base_url, client_key) = if env_value == "prod" {
        (
            "https://pointr-api.duckdns.org",
            "G1m1PJEVokCsMOchwHYzGKORc374rxeW9oLGuNhyQwk",
        )
    } else {
        ("http://localhost:8000", "")
    };
    println!("cargo:rustc-env=POINTR_API_BASE_URL={}", base_url);
    println!("cargo:rustc-env=POINTR_CLIENT_KEY={}", client_key);
    println!("cargo:rerun-if-changed=../../.env");

    tauri_build::build()
}
