use zed_extension_api::{self as zed, LanguageServerId, Result};

struct KotlinLspExtension;

impl zed::Extension for KotlinLspExtension {
    fn new() -> Self {
        KotlinLspExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = worktree
            .which("kmp-lsp")
            .ok_or_else(|| "kmp-lsp not found on PATH. Install it with: cargo install kmp-lsp".to_string())?;

        Ok(zed::Command {
            command: binary,
            args: vec!["--stdio".to_string()],
            env: Default::default(),
        })
    }
}

zed::register_extension!(KotlinLspExtension);
