use std::path::Path;
use std::process::Command;

/// Variables that would lead a launched `orx` or agent to the developer's own
/// state, credentials or session: `config::REDIRECT_VARS` and `PROVIDER_VARS`.
const CLEARED: [&str; 52] = [
    "ORX_DATA_DIR",
    "ORX_CACHE_DIR",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "KIMI_CODE_HOME",
    "KIMI_SHARE_DIR",
    "MINIMAX_DATA_DIR",
    "MAVIS_DATA_DIR",
    "ZCODE_HOME",
    "GROK_HOME",
    "GROK_AUTH_PATH",
    "MODAL_CONFIG_PATH",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "CLAUDE_CODE_PROMPT_CACHE_TTL",
    "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENROUTER_API_KEY",
    "CURSOR_API_KEY",
    "DEEPSEEK_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROQ_API_KEY",
    "KIMI_API_KEY",
    "XAI_API_KEY",
    "GROK_AUTH_PROVIDER_COMMAND",
    "GROK_DISABLE_API_KEY_AUTH",
    "GROK_OAUTH2_CLIENT_ID",
    "GROK_OAUTH2_ISSUER",
    "GROK_OIDC_CLIENT_ID",
    "GROK_OIDC_ISSUER",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PASSWORD",
    "HF_TOKEN",
    "HF_ENDPOINT",
    "MODAL_PROFILE",
    "MODAL_TOKEN_ID",
    "MODAL_TOKEN_SECRET",
    "TINKER_API_KEY",
    "VERCEL_TOKEN",
    "VERCEL_OIDC_TOKEN",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_THREAD_ID",
    "OPENCODE",
    "CURSOR_AGENT",
    "ORX_CHAT_SESSION_ID",
    "ORX_CHAT_HARNESS",
    "ORX_LOCAL_SESSION",
];

/// Points every home `orx` or an agent resolves inside `home`, whatever the
/// developer's shell exports, and on Unix keeps PATH to the system directories
/// so `orx up` never finds their agents: a launched `orx` has no `cfg(test)`
/// guard. Call it before the launcher's own `.env`s, which then win.
pub fn isolate_home<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    for var in CLEARED {
        command.env_remove(var);
    }
    #[cfg(unix)]
    command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("HF_HOME", home.join(".cache/huggingface"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", home.join(".claude"))
        .env("CURSOR_CONFIG_DIR", home.join(".cursor"))
        .env("CURSOR_DATA_DIR", home.join(".cursor"))
        .env(
            "OPENCODE_DB",
            home.join(".local/share/opencode/opencode.db"),
        )
}
