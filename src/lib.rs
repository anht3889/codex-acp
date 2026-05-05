//! Codex ACP - An Agent Client Protocol implementation for Codex.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use agent_client_protocol::ByteStreams;
use codex_cloud_requirements::cloud_requirements_loader_for_storage;
use codex_config::TomlValue;
use codex_core::config::{Config, ConfigBuilder, ConfigOverrides};
use codex_login::default_client::set_default_client_residency_requirement;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing_subscriber::EnvFilter;

mod codex_agent;
mod thread;

const MCP_OAUTH_CREDENTIALS_STORE_KEY: &str = "mcp_oauth_credentials_store";
const MCP_OAUTH_CREDENTIALS_STORE_FILE: &str = "file";

/// Run the Codex ACP agent.
///
/// This sets up an ACP agent that communicates over stdio, bridging
/// the ACP protocol with the existing codex-rs infrastructure.
///
/// # Errors
///
/// If unable to parse the config or start the program.
pub async fn run_main(
    codex_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
) -> std::io::Result<()> {
    // Install a simple subscriber so `tracing` output is visible.
    // Users can control the log level with `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Parse CLI overrides and load configuration
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;
    let cli_kv_overrides = with_acp_default_cli_overrides(cli_kv_overrides);

    let config_overrides = ConfigOverrides {
        codex_linux_sandbox_exe: codex_linux_sandbox_exe.clone(),
        ..ConfigOverrides::default()
    };

    let base_config = load_config(cli_kv_overrides.clone(), config_overrides.clone()).await?;
    let cloud_requirements = cloud_requirements_loader_for_storage(
        base_config.codex_home.to_path_buf(),
        /*enable_codex_api_key_env*/ false,
        base_config.cli_auth_credentials_store_mode,
        base_config.chatgpt_base_url.clone(),
    )
    .await;
    let config = ConfigBuilder::default()
        .cli_overrides(cli_kv_overrides)
        .harness_overrides(config_overrides)
        .cloud_requirements(cloud_requirements)
        .build()
        .await
        .map_err(config_load_error)?;
    set_default_client_residency_requirement(config.enforce_residency.value());

    let agent = Arc::new(codex_agent::CodexAgent::new(config, codex_linux_sandbox_exe).await?);

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    agent
        .serve(ByteStreams::new(stdout, stdin))
        .await
        .map_err(|e| std::io::Error::other(format!("ACP error: {e}")))?;

    Ok(())
}

async fn load_config(
    cli_kv_overrides: Vec<(String, TomlValue)>,
    config_overrides: ConfigOverrides,
) -> std::io::Result<Config> {
    Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, config_overrides)
        .await
        .map_err(config_load_error)
}

fn config_load_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("error loading config: {error}"),
    )
}

fn with_acp_default_cli_overrides(
    mut cli_kv_overrides: Vec<(String, TomlValue)>,
) -> Vec<(String, TomlValue)> {
    if !cli_kv_overrides
        .iter()
        .any(|(key, _)| key == MCP_OAUTH_CREDENTIALS_STORE_KEY)
    {
        cli_kv_overrides.push((
            MCP_OAUTH_CREDENTIALS_STORE_KEY.to_string(),
            TomlValue::String(MCP_OAUTH_CREDENTIALS_STORE_FILE.to_string()),
        ));
    }
    cli_kv_overrides
}

// Re-export the MCP server types for compatibility
pub use codex_mcp_server::{
    CodexToolCallParam, CodexToolCallReplyParam, ExecApprovalElicitRequestParams,
    ExecApprovalResponse, PatchApprovalElicitRequestParams, PatchApprovalResponse,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_defaults_mcp_oauth_credentials_to_file() {
        let overrides = with_acp_default_cli_overrides(Vec::new());

        assert_eq!(
            overrides,
            vec![(
                MCP_OAUTH_CREDENTIALS_STORE_KEY.to_string(),
                TomlValue::String(MCP_OAUTH_CREDENTIALS_STORE_FILE.to_string()),
            )]
        );
    }

    #[test]
    fn acp_default_does_not_override_explicit_mcp_oauth_credentials_store() {
        let explicit_override = (
            MCP_OAUTH_CREDENTIALS_STORE_KEY.to_string(),
            TomlValue::String("keyring".to_string()),
        );
        let overrides = with_acp_default_cli_overrides(vec![explicit_override.clone()]);

        assert_eq!(overrides, vec![explicit_override]);
    }
}
