//! The Burdin extension for Zed (spec/lsp.md, LS20). It starts `burdin lsp` and passes the `burdin` settings (LS5);
//! everything else is the language server's.
//!
//! The server is `burdin` on the worktree's `PATH`, or the `lsp.burdin.binary.path` setting. The settings come from
//! `lsp.burdin.settings` in Zed's settings, as the object under `burdin` (`{ "inlayHints": { "types": false } }`).

use zed_extension_api::{self as zed, LanguageServerId, Result, serde_json, settings::LspSettings};

const SERVER: &str = "burdin";

struct BurdinExtension;

impl BurdinExtension {
    /// The `burdin` settings: `lsp.burdin.settings`, with `path` filled in from `binary.path` when only that is set,
    /// so the test lenses run the same `burdin` the server is.
    fn settings(worktree: &zed::Worktree) -> serde_json::Value {
        let lsp = LspSettings::for_worktree(SERVER, worktree).unwrap_or_default();
        let mut settings = lsp.settings.unwrap_or_else(|| serde_json::json!({}));
        let binary = lsp.binary.and_then(|b| b.path);
        if let (Some(object), Some(path)) = (settings.as_object_mut(), binary) {
            object.entry("path").or_insert(serde_json::Value::String(path));
        }
        settings
    }
}

impl zed::Extension for BurdinExtension {
    fn new() -> Self {
        BurdinExtension
    }

    fn language_server_command(&mut self, _id: &LanguageServerId, worktree: &zed::Worktree) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER, worktree).ok().and_then(|s| s.binary);
        let configured = binary.as_ref().and_then(|b| b.path.clone());
        let command = match configured {
            Some(path) => path,
            None => worktree.which(SERVER).ok_or_else(|| {
                "`burdin` is not on PATH: install Burdin, or set `lsp.burdin.binary.path` in Zed's settings".to_string()
            })?,
        };
        let args = binary.as_ref().and_then(|b| b.arguments.clone()).unwrap_or_else(|| vec!["lsp".to_string()]);
        let mut env = worktree.shell_env();
        if let Some(extra) = binary.and_then(|b| b.env) {
            env.extend(extra);
        }
        Ok(zed::Command { command, args, env })
    }

    /// `initializationOptions` is the object under `burdin` itself (LS5), unless the user gave their own.
    fn language_server_initialization_options(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let given = LspSettings::for_worktree(SERVER, worktree).ok().and_then(|s| s.initialization_options);
        Ok(Some(given.unwrap_or_else(|| Self::settings(worktree))))
    }

    /// `workspace/didChangeConfiguration` and `workspace/configuration` read the settings under `burdin`, so a change
    /// in Zed's settings reaches the server without a restart.
    fn language_server_workspace_configuration(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(serde_json::json!({ "burdin": Self::settings(worktree) })))
    }
}

zed::register_extension!(BurdinExtension);
