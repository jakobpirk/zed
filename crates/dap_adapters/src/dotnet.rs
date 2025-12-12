use crate::*;
use anyhow::{Context as _, bail, Result};
use dap::{DebugRequest, StartDebuggingRequestArguments, adapters::DebugTaskDefinition};
use gpui::SharedString;
use paths::debug_adapters_dir;
use serde_json::Value;
use smol::lock::OnceCell;
use std::path::{Path, PathBuf};
use util::command::new_smol_command;

/// vsdbg is Microsoft's official .NET debugger adapter
/// Supports .NET Framework, .NET Core, and .NET 5+
#[derive(Default)]
pub(crate) struct DotNetDebugAdapter {
    vsdbg_path: OnceCell<Result<std::sync::Arc<Path>, String>>,
}

impl DotNetDebugAdapter {
    const ADAPTER_NAME: &'static str = "vsdbg";
    const DEBUG_ADAPTER_NAME: DebugAdapterName =
        DebugAdapterName(SharedString::new_static(Self::ADAPTER_NAME));

    /// Get vsdbg binary path
    /// Checks for vsdbg in PATH or in the cached debug adapters directory
    async fn fetch_vsdbg(&self) -> Result<std::sync::Arc<Path>> {
        // First, check if vsdbg is in PATH
        let which_result = new_smol_command("which")
            .arg(if cfg!(windows) { "vsdbg.exe" } else { "vsdbg" })
            .output()
            .await;

        if let Ok(output) = which_result {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let vsdbg_path = PathBuf::from(path_str);
                    if vsdbg_path.exists() {
                        log::info!("Found vsdbg in PATH: {}", vsdbg_path.display());
                        return Ok(vsdbg_path.into());
                    }
                }
            }
        }

        // Check cached location
        let cache_dir = debug_adapters_dir().join(Self::ADAPTER_NAME);
        let binary_name = if cfg!(windows) { "vsdbg.exe" } else { "vsdbg" };
        let cached_binary = cache_dir.join(binary_name);

        if cached_binary.exists() {
            log::info!("Found cached vsdbg at {}", cached_binary.display());
            return Ok(cached_binary.into());
        }

        // vsdbg not found
        bail!(
            "vsdbg not found. Please install .NET SDK or download vsdbg manually.\n\
             To install: https://github.com/microsoft/vscode-csharp or dotnet install tool"
        )
    }

    /// Get or fetch the vsdbg binary path
    async fn vsdbg_path(&self) -> Result<std::sync::Arc<Path>, String> {
        self.vsdbg_path
            .get_or_try_insert(self.fetch_vsdbg().await)
            .clone()
    }

    /// Generate request arguments for launching a .NET application
    async fn request_args(
        &self,
        _delegate: &std::sync::Arc<dyn DapDelegate>,
        task_definition: &DebugTaskDefinition,
    ) -> Result<StartDebuggingRequestArguments> {
        let request = if task_definition
            .config
            .get("request")
            .and_then(|v| v.as_str())
            == Some("attach")
        {
            "attach".to_string()
        } else {
            "launch".to_string()
        };

        let mut configuration = task_definition.config.clone();

        // Set console if not provided
        if configuration.get("console").is_none() {
            configuration["console"] = Value::String("integratedTerminal".to_string());
        }

        // Ensure program path is set for launch requests
        if request == "launch" && configuration.get("program").is_none() {
            bail!("'program' is required for launch requests");
        }

        Ok(StartDebuggingRequestArguments {
            configuration,
            request,
        })
    }
}

#[async_trait(?Send)]
impl DebugAdapter for DotNetDebugAdapter {
    fn name(&self) -> DebugAdapterName {
        Self::DEBUG_ADAPTER_NAME
    }

    fn dap_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["coreclr"],
                    "description": "Type of debugger",
                    "default": "coreclr"
                },
                "request": {
                    "type": "string",
                    "enum": ["launch", "attach"],
                    "description": "Launch or attach to a running process"
                },
                "name": {
                    "type": "string",
                    "description": "The name of the debug session"
                },
                "program": {
                    "type": "string",
                    "description": "Path to the .NET executable or DLL to debug"
                },
                "args": {
                    "type": ["array"],
                    "items": { "type": "string" },
                    "description": "Command line arguments to pass to the program"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory of the program"
                },
                "stopAtEntry": {
                    "type": "boolean",
                    "description": "Stop at the first line of the program",
                    "default": false
                },
                "console": {
                    "type": "string",
                    "enum": ["integratedTerminal", "externalTerminal", "internalConsole"],
                    "description": "Which console to use"
                },
                "processId": {
                    "type": ["string", "integer"],
                    "description": "Process ID to attach to (for attach requests)"
                }
            }
        })
    }

    async fn get_binary(
        &self,
        _delegate: &std::sync::Arc<dyn DapDelegate>,
        _config: &DebugTaskDefinition,
        user_installed_path: Option<PathBuf>,
        _user_args: Option<Vec<String>>,
        _user_env: Option<std::collections::HashMap<String, String>>,
        _cx: &mut gpui::AsyncApp,
    ) -> Result<DebugAdapterBinary> {
        let binary_path = if let Some(path) = user_installed_path {
            path.into()
        } else {
            self.vsdbg_path().await.map_err(|e| anyhow::anyhow!(e))?
        };

        Ok(DebugAdapterBinary::Path(binary_path))
    }

    async fn create_request(
        &self,
        delegate: &std::sync::Arc<dyn DapDelegate>,
        config: &DebugTaskDefinition,
    ) -> Result<DebugRequest> {
        let args = self.request_args(delegate, config).await?;
        Ok(DebugRequest::Launch(
            serde_json::to_value(args).context("Failed to serialize debug arguments")?,
        ))
    }
}
