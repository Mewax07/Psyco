use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

const SERVER_BINARY: &str = "psyco-lsp";

struct PsycoExtension;

impl zed::Extension for PsycoExtension {
    fn new() -> Self {
        PsycoExtension
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.binary);

        let mut env = worktree.shell_env();
        if let Some(extra) = binary.as_ref().and_then(|b| b.env.clone()) {
            env.extend(extra);
        }
        let args = binary
            .as_ref()
            .and_then(|b| b.arguments.clone())
            .unwrap_or_default();

        let windows = matches!(zed::current_platform(), (zed::Os::Windows, _));
        let exe = if windows {
            format!("{SERVER_BINARY}.exe")
        } else {
            SERVER_BINARY.to_string()
        };
        let command = binary
            .and_then(|b| b.path)
            .or_else(|| worktree.which(SERVER_BINARY))
            .or_else(|| worktree.which(&exe))
            .or_else(|| cargo_bin(&env, &exe))
            .ok_or_else(|| {
                format!(
                    "'{SERVER_BINARY}' was not found in PATH. Install it with \
                     `cargo install --path zed-psyco/psyco-lsp` or set \
                     lsp.{SERVER_BINARY}.binary.path in your Zed settings."
                )
            })?;

        Ok(zed::Command { command, args, env })
    }
}

/// Where `cargo install` puts binaries, for when Zed does not see `~/.cargo/bin`
/// in its PATH (common on Windows).
fn cargo_bin(env: &[(String, String)], exe: &str) -> Option<String> {
    let var = |name: &str| {
        env.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.replace('\\', "/"))
            .filter(|v| !v.is_empty())
    };
    let cargo_home = var("CARGO_HOME").or_else(|| {
        var("USERPROFILE")
            .or_else(|| var("HOME"))
            .map(|home| format!("{home}/.cargo"))
    })?;
    Some(format!("{cargo_home}/bin/{exe}"))
}

zed::register_extension!(PsycoExtension);
