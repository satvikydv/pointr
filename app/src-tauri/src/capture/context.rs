use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

const MAX_TITLE_LEN: usize = 80;

#[derive(Debug, Clone)]
pub struct AppContext {
    pub app_name: String,
    pub window_title: String,
    pub language: Option<String>,
    /// Raw HWND of the window that was in the foreground at capture time —
    /// stored as isize (not the windows-rs HWND type) so it's plain, Send,
    /// storable in Tauri-managed state. Used later to restore focus there
    /// before an agent's "type into the focused field" action executes:
    /// by the time the user has asked a question and confirmed the action,
    /// Pointr's own window has taken OS focus, so without this the keystrokes
    /// would go nowhere useful.
    pub hwnd: isize,
}

impl AppContext {
    /// Single line handed to the backend as the active-window fact — plain
    /// text so the prompt states it outright instead of leaving the model to
    /// infer the app/language from pixels.
    pub fn describe(&self) -> String {
        match &self.language {
            Some(lang) if !self.window_title.is_empty() => {
                format!("{} — {} ({})", self.app_name, self.window_title, lang)
            }
            _ if !self.window_title.is_empty() && self.window_title != self.app_name => {
                format!("{} — {}", self.app_name, self.window_title)
            }
            _ => self.app_name.clone(),
        }
    }
}

/// Best-effort: any failure along the way (no foreground window, process
/// access denied, etc.) just falls back to an empty context rather than
/// failing the whole capture over a nice-to-have.
pub fn get_foreground_app_context() -> AppContext {
    try_get_foreground_app_context().unwrap_or(AppContext {
        app_name: "Unknown".to_string(),
        window_title: String::new(),
        language: None,
        hwnd: 0,
    })
}

fn try_get_foreground_app_context() -> anyhow::Result<AppContext> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return Err(anyhow::anyhow!("no foreground window"));
        }

        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title_buf);
        let raw_title = String::from_utf16_lossy(&title_buf[..len.max(0) as usize]);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == 0 {
            return Ok(AppContext {
                app_name: "Unknown".to_string(),
                window_title: truncate(&raw_title),
                language: None,
                hwnd: hwnd.0 as isize,
            });
        }

        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid)?;
        let mut exe_buf = [0u16; 1024];
        let mut exe_len = exe_buf.len() as u32;
        let exe_path = if QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(exe_buf.as_mut_ptr()),
            &mut exe_len,
        )
        .is_ok()
        {
            String::from_utf16_lossy(&exe_buf[..exe_len as usize])
        } else {
            String::new()
        };
        let _ = CloseHandle(process);

        let app_name = friendly_app_name(&exe_path);

        let (window_title, language) = if is_code_editor(&app_name) {
            match parse_editor_file(&raw_title) {
                Some((filename, lang)) => (filename, lang),
                None => (truncate(&raw_title), None),
            }
        } else {
            (truncate(&strip_app_suffix(&raw_title, &app_name)), None)
        };

        Ok(AppContext {
            app_name,
            window_title,
            language,
            hwnd: hwnd.0 as isize,
        })
    }
}

/// Browser (and some other apps') window titles conventionally end with
/// " - <product name>" (e.g. "Reverse Engineering HeyClicky - Brave"). That
/// duplicates the app_name describe() already prefixes, so strip it here —
/// covers both the friendly name we picked and the vendor's own full name.
fn strip_app_suffix(title: &str, app_name: &str) -> String {
    let known_full_names: &[(&str, &str)] = &[
        ("Chrome", "Google Chrome"),
        ("Edge", "Microsoft Edge"),
        ("Firefox", "Mozilla Firefox"),
    ];
    let full_name = known_full_names
        .iter()
        .find(|(short, _)| *short == app_name)
        .map(|(_, full)| *full);

    for suffix in [Some(app_name), full_name].into_iter().flatten() {
        let marker = format!(" - {}", suffix);
        if let Some(stripped) = title.strip_suffix(marker.as_str()) {
            return stripped.trim().to_string();
        }
    }
    title.to_string()
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_TITLE_LEN {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(MAX_TITLE_LEN).collect();
        t.push('…');
        t
    }
}

fn friendly_app_name(exe_path: &str) -> String {
    let stem = std::path::Path::new(exe_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let lower = stem.to_lowercase();
    let known: &[(&str, &str)] = &[
        ("code", "VS Code"),
        ("devenv", "Visual Studio"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("brave", "Brave"),
        ("firefox", "Firefox"),
        ("explorer", "File Explorer"),
        ("notepad", "Notepad"),
        ("notepad++", "Notepad++"),
        ("photoshop", "Photoshop"),
        ("illustrator", "Illustrator"),
        ("winword", "Word"),
        ("excel", "Excel"),
        ("powerpnt", "PowerPoint"),
        ("slack", "Slack"),
        ("discord", "Discord"),
        ("windowsterminal", "Windows Terminal"),
        ("cmd", "Command Prompt"),
        ("powershell", "PowerShell"),
        ("pwsh", "PowerShell"),
        ("postman", "Postman"),
        ("figma", "Figma"),
        ("spotify", "Spotify"),
        ("sublime_text", "Sublime Text"),
        ("idea64", "IntelliJ IDEA"),
        ("pycharm64", "PyCharm"),
        ("webstorm64", "WebStorm"),
        ("outlook", "Outlook"),
        ("teams", "Microsoft Teams"),
        ("zoom", "Zoom"),
    ];

    known
        .iter()
        .find(|(k, _)| *k == lower)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| title_case(&stem))
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

const CODE_EDITORS: &[&str] = &[
    "VS Code",
    "Visual Studio",
    "Sublime Text",
    "Notepad++",
    "IntelliJ IDEA",
    "PyCharm",
    "WebStorm",
];

fn is_code_editor(app_name: &str) -> bool {
    CODE_EDITORS.contains(&app_name)
}

/// Editor window titles are conventionally "filename - workspace - App Name"
/// (VS Code, Visual Studio, Sublime, etc.), optionally prefixed with a
/// "modified" dot marker on the filename. Pull the filename back out and
/// guess a language from its extension.
fn parse_editor_file(window_title: &str) -> Option<(String, Option<String>)> {
    let first_segment = window_title.split(" - ").next()?.trim();
    let filename = first_segment.trim_start_matches('●').trim().to_string();
    if filename.is_empty() {
        return None;
    }

    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    let language = ext.and_then(|e| language_for_extension(&e));
    Some((filename, language))
}

fn language_for_extension(ext: &str) -> Option<String> {
    let lang = match ext {
        "rs" => "Rust",
        "py" => "Python",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "m" => "Objective-C",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "md" => "Markdown",
        "sql" => "SQL",
        "sh" | "bash" => "Shell",
        "ps1" => "PowerShell",
        "xml" => "XML",
        "lua" => "Lua",
        "r" => "R",
        "dart" => "Dart",
        "vue" => "Vue",
        _ => return None,
    };
    Some(lang.to_string())
}
