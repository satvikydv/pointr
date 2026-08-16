/// Single place for the backend URL and shared client key, picked at
/// compile time from POINTR_ENV in app/src-tauri/.env (read by build.rs,
/// defaults to "local" if that file/key is missing). Set POINTR_ENV=prod
/// to build against the deployed backend; leave it unset/"local" for
/// everyday `cargo tauri dev` against a locally-running docker-compose
/// backend. The key matches POINTR_CLIENT_KEY in the server's own .env
/// (see backend/app/security.py) — a deterrent against random scraping on
/// a publicly-reachable, quota-metered backend, not real per-user auth (it
/// ships inside this binary in prod builds). Empty locally, matching the
/// server's no-op behavior when POINTR_CLIENT_KEY isn't set.
pub const API_BASE_URL: &str = env!("POINTR_API_BASE_URL");
pub const CLIENT_KEY: &str = env!("POINTR_CLIENT_KEY");

#[derive(serde::Serialize)]
pub struct ApiConfig {
    pub base_url: String,
    pub client_key: String,
}

/// Lets the JS side (which has no build step / can't read POINTR_ENV
/// itself) follow whatever this Rust build was compiled for, instead of
/// keeping a second hardcoded copy of the same switch in main.js.
#[tauri::command]
pub fn get_api_config() -> ApiConfig {
    ApiConfig {
        base_url: API_BASE_URL.to_string(),
        client_key: CLIENT_KEY.to_string(),
    }
}
