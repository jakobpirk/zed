use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use collections::FxHashMap;
use dap::{DapLocator, DebugRequest, adapters::DebugAdapterName};
use gpui::SharedString;
use serde_json::json;
use smol::io::AsyncReadExt;
use smol::process::Stdio;
use std::path::{Path, PathBuf};
use task::{BuildTaskDefinition, DebugScenario, LaunchRequest, ShellBuilder, SpawnInTerminal, TaskTemplate};
use util::command::new_smol_command;

/// Debug locator for .NET projects
/// Converts "dotnet run" tasks to debug configurations
/// Parses build output to find the executable DLL path
pub(crate) struct DotNetLocator;

#[async_trait]
impl DapLocator for DotNetLocator {
    fn name(&self) -> SharedString {
        SharedString::new_static("dotnet-locator")
    }

    async fn create_scenario(
        &self,
        build_config: &TaskTemplate,
        resolved_label: &str,
        adapter: &DebugAdapterName,
    ) -> Option<DebugScenario> {
        // Only handle dotnet commands
        if build_config.command != "dotnet" {
            return None;
        }

        let mut task_template = build_config.clone();
        let Some(dotnet_action) = task_template.args.first_mut() else {
            return None;
        };

        // Determine what to do based on the dotnet subcommand
        match dotnet_action.as_str() {
            "run" | "r" => {
                // Convert "dotnet run" to "dotnet build"
                // The locator's run() method will find the executable
                *dotnet_action = "build".to_owned();
            }
            "test" => {
                // Test debugging - build without running
                // Could skip the build if --no-build is present
                if !task_template.args.contains(&"--no-build".to_owned()) {
                    // Tests typically don't need building separately
                    return None;
                }
            }
            "build" => {
                // Already a build command, can use it
            }
            _ => {
                // Other commands (clean, restore, etc.) - not debuggable
                return None;
            }
        }

        // Return a debug scenario that will call run() after building
        Some(DebugScenario {
            adapter: adapter.0.clone(),
            label: resolved_label.to_string().into(),
            build: Some(BuildTaskDefinition::Template {
                task_template,
                locator_name: Some(self.name()),
            }),
            config: json!({
                "type": "coreclr",
                "request": "launch"
            }),
            tcp_connection: None,
        })
    }

    async fn run(&self, build_config: SpawnInTerminal) -> Result<DebugRequest> {
        let cwd = build_config
            .cwd
            .clone()
            .context("Working directory required for dotnet build")?;

        // Build the dotnet command with output path generation
        let builder = ShellBuilder::new(&build_config.shell, cfg!(windows)).non_interactive();
        let (program, mut args) = builder.build(
            Some("dotnet".into()),
            &build_config
                .args
                .iter()
                .cloned()
                .take_while(|arg| arg != "--")
                .collect::<Vec<_>>(),
        );

        // Add flags to get full paths and verbose output
        args.push("--no-restore".to_string());
        args.push("/p:GenerateFullPaths=true".to_string());
        args.push("-v:q".to_string()); // Quiet verbosity to reduce output noise

        log::info!("Running dotnet build: {} {:?}", program, args);

        // Execute the build
        let mut child = new_smol_command(&program)
            .args(&args)
            .envs(build_config.env.iter().map(|(k, v)| (k.clone(), v.clone())))
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn dotnet build")?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let Some(mut out) = child.stdout.take() {
            out.read_to_string(&mut stdout).await.ok();
        }
        if let Some(mut err) = child.stderr.take() {
            err.read_to_string(&mut stderr).await.ok();
        }

        let status = child.status().await.context("Build process failed")?;

        if !status.success() {
            bail!(
                "dotnet build failed with exit code {:?}\nstderr: {}",
                status.code(),
                stderr
            );
        }

        // Parse the output to find the built DLL path
        let dll_path = find_dotnet_output_assembly(&stdout, &cwd)?;

        log::info!("Found output assembly: {}", dll_path);

        // Create the debug launch request
        let launch_request = LaunchRequest {
            program: dll_path,
            cwd: Some(cwd),
            args: vec![],
            env: FxHashMap::default(),
        };

        Ok(DebugRequest::Launch(launch_request))
    }
}

/// Parse dotnet build output to find the compiled assembly path
/// Dotnet outputs lines like: "MyApp -> /path/to/bin/Debug/net6.0/MyApp.dll"
fn find_dotnet_output_assembly(output: &str, cwd: &std::path::Path) -> Result<String> {
    // Look for the pattern: "ProjectName -> /path/to/assembly"
    for line in output.lines() {
        if let Some(arrow_pos) = line.find("->") {
            let assembly_part = line[arrow_pos + 2..].trim();

            // Check if this looks like a .dll or .exe file
            if assembly_part.ends_with(".dll") || assembly_part.ends_with(".exe") {
                // Make the path absolute if it's relative
                let path = std::path::Path::new(assembly_part);
                let absolute_path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };

                if absolute_path.exists() {
                    return Ok(absolute_path.to_string_lossy().to_string());
                }
            }
        }
    }

    // Fallback: try to find a recently modified DLL in common output paths
    let possible_output_dirs = vec![
        cwd.join("bin/Debug"),
        cwd.join("bin/Release"),
        cwd.join("bin/Debug/net6.0"),
        cwd.join("bin/Debug/net5.0"),
        cwd.join("bin/Debug/net8.0"),
        cwd.join("bin/Release/net6.0"),
        cwd.join("bin/Release/net5.0"),
        cwd.join("bin/Release/net8.0"),
    ];

    for dir in possible_output_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.ends_with(".dll") && metadata.is_file() {
                            return Ok(entry.path().to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    bail!(
        "Could not find compiled assembly in dotnet build output.\n\
         Build output was:\n{}",
        output
    )
}

/// Find the .sln file in or above the given directory
fn find_solution_file(dir: &Path) -> Option<PathBuf> {
    // Search current directory and parent directories for .sln files
    let mut current = dir.to_path_buf();

    loop {
        // Check for any .sln file in current directory
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if name.ends_with(".sln") {
                        return Some(entry.path());
                    }
                }
            }
        }

        // Move up to parent directory
        if !current.pop() {
            break;
        }
    }

    None
}

/// Try to find the startup project path from a solution file
/// Returns the path to the startup project's directory
fn find_startup_project_from_solution(solution_path: &Path, solution_dir: &Path) -> Option<PathBuf> {
    // Read the solution file
    let content = std::fs::read_to_string(solution_path).ok()?;

    let mut first_exe_project = None;

    // Parse solution file to find projects
    for line in content.lines() {
        let line = line.trim();

        // Look for Project entries: Project("{type-guid}") = "name", "path", "{guid}"
        if line.starts_with("Project(\"") {
            if let Some(project_info) = extract_project_info(line) {
                let project_path = solution_dir.join(&project_info.0);

                // Check if this is likely an executable project (not test, not library by name heuristic)
                if !project_info.1.contains("Test") && first_exe_project.is_none() {
                    first_exe_project = Some(project_path.clone());
                }
            }
        }
    }

    // Prefer the first non-test project found
    first_exe_project
}

/// Extract project path and name from a Project line
/// Format: Project("{type-guid}") = "name", "path", "{guid}"
fn extract_project_info(line: &str) -> Option<(String, String)> {
    if let Some(after_equals) = line.split('=').nth(1) {
        let parts: Vec<&str> = after_equals
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();

        if parts.len() >= 2 {
            let name = parts[0].to_string();
            let path = parts[1].to_string();
            return Some((path, name));
        }
    }
    None
}
