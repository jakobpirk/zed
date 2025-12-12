use anyhow::Result;
use async_trait::async_trait;
use collections::HashMap;
use gpui::{App, SharedString, Task};
use language::{
    ContextLocation, ContextProvider, LanguageToolchainStore, ManifestName,
    ManifestProvider, ManifestQuery,
};
use std::{hash::{Hash, Hasher}, path::{Path, PathBuf}, sync::Arc};
use std::collections::hash_map::DefaultHasher;
use task::{TaskTemplate, TaskTemplates, TaskVariables};
use util::rel_path::RelPath;
use util::paths::PathStyle;

/// Manifest provider for .csproj files
/// Detects .NET project files and returns their directory as the project root
pub struct CsprojManifestProvider;

impl ManifestProvider for CsprojManifestProvider {
    fn name(&self) -> ManifestName {
        SharedString::new_static(".csproj").into()
    }

    fn search(&self, query: ManifestQuery) -> Option<Arc<RelPath>> {
        let ManifestQuery {
            path,
            depth,
            delegate,
        } = query;

        // Walk up the directory tree looking for .csproj files
        for ancestor_path in path.ancestors().take(depth) {
            // Check for common .csproj patterns
            // We check for specific filenames since the delegate only supports specific path checks

            // First, try to find a .csproj with the same name as the directory
            if let Some(dir_name) = ancestor_path.file_name() {
                let csproj_name = format!("{}.csproj", dir_name);
                if let Ok(rel_path) = RelPath::new(Path::new(&csproj_name), PathStyle::Posix) {
                    let project_path = ancestor_path.join(rel_path.as_ref());
                    if delegate.exists(&project_path, Some(false)) {
                        return Some(Arc::from(ancestor_path));
                    }
                }
            }

            // Also check for common project file names
            for common_name in &["project.csproj", "app.csproj", "web.csproj"] {
                if let Ok(rel_path) = RelPath::new(Path::new(common_name), PathStyle::Posix) {
                    let project_path = ancestor_path.join(rel_path.as_ref());
                    if delegate.exists(&project_path, Some(false)) {
                        return Some(Arc::from(ancestor_path));
                    }
                }
            }
        }

        None
    }
}

/// Manifest provider for .sln files
/// Detects .NET solution files and returns their directory as the solution root
pub struct SolutionManifestProvider;

impl ManifestProvider for SolutionManifestProvider {
    fn name(&self) -> ManifestName {
        SharedString::new_static(".sln").into()
    }

    fn search(&self, query: ManifestQuery) -> Option<Arc<RelPath>> {
        let ManifestQuery {
            path,
            depth,
            delegate,
        } = query;

        // Walk up the directory tree looking for .sln files
        for ancestor_path in path.ancestors().take(depth) {
            // Check for common .sln patterns
            // First, try to find a .sln with the same name as the directory
            if let Some(dir_name) = ancestor_path.file_name() {
                let sln_name = format!("{}.sln", dir_name);
                if let Ok(rel_path) = RelPath::new(Path::new(&sln_name), PathStyle::Posix) {
                    let solution_path = ancestor_path.join(rel_path.as_ref());
                    if delegate.exists(&solution_path, Some(false)) {
                        return Some(Arc::from(ancestor_path));
                    }
                }
            }

            // Also check for common solution file names
            for common_name in &["solution.sln"] {
                if let Ok(rel_path) = RelPath::new(Path::new(common_name), PathStyle::Posix) {
                    let solution_path = ancestor_path.join(rel_path.as_ref());
                    if delegate.exists(&solution_path, Some(false)) {
                        return Some(Arc::from(ancestor_path));
                    }
                }
            }
        }

        None
    }
}

/// Context provider for C# projects
/// Provides task variables and task templates for .NET builds
pub(crate) struct CSharpContextProvider;

#[async_trait(?Send)]
impl ContextProvider for CSharpContextProvider {
    fn build_context(
        &self,
        variables: &TaskVariables,
        _location: ContextLocation<'_>,
        _project_env: Option<HashMap<String, String>>,
        _language_toolchain_store: Arc<dyn LanguageToolchainStore>,
        _cx: &mut App,
    ) -> Task<Result<TaskVariables>> {
        // For now, just return the provided variables without modification
        // A full implementation would parse .csproj files to extract actual values
        Task::ready(Ok(variables.clone()))
    }

    fn associated_tasks(
        &self,
        _file: Option<Arc<dyn language::File>>,
        _cx: &App,
    ) -> Task<Option<TaskTemplates>> {
        // Provide default task templates for common dotnet operations
        let templates = TaskTemplates(vec![
            TaskTemplate {
                label: "dotnet: build".into(),
                command: "dotnet".into(),
                args: vec!["build".into()],
                ..Default::default()
            },
            TaskTemplate {
                label: "dotnet: clean".into(),
                command: "dotnet".into(),
                args: vec!["clean".into()],
                ..Default::default()
            },
            TaskTemplate {
                label: "dotnet: test".into(),
                command: "dotnet".into(),
                args: vec!["test".into()],
                ..Default::default()
            },
            TaskTemplate {
                label: "dotnet: run".into(),
                command: "dotnet".into(),
                args: vec!["run".into()],
                ..Default::default()
            },
        ]);
        Task::ready(Some(templates))
    }
}

/// Represents a NuGet package reference in a project
#[derive(Debug, Clone)]
pub struct NuGetPackage {
    /// Package ID (e.g., "Newtonsoft.Json")
    pub id: String,
    /// Package version (e.g., "13.0.1")
    pub version: Option<String>,
}

/// Represents a project in a .NET solution
#[derive(Debug, Clone)]
pub struct SolutionProject {
    /// Project name
    pub name: String,
    /// Path to .csproj file relative to solution directory
    pub path: PathBuf,
    /// Project GUID (unique identifier)
    pub guid: String,
    /// Project type GUID (e.g., C#, VB.NET, C++, etc.)
    pub type_guid: String,
    /// NuGet packages referenced by this project
    pub packages: Vec<NuGetPackage>,
}

/// Represents a parsed .NET solution (.sln) file
#[derive(Debug, Clone)]
pub struct SolutionFile {
    /// Path to the solution file
    pub path: PathBuf,
    /// All projects in the solution
    pub projects: Vec<SolutionProject>,
    /// Solution configurations (e.g., Debug, Release)
    pub configurations: Vec<String>,
    /// Startup project GUID (if specified)
    pub startup_project: Option<String>,
}

impl SolutionFile {
    /// Parse a .NET solution file (.sln or .slnx format)
    pub fn parse(content: &str, base_dir: &Path) -> Result<Self> {
        // Check if it's XML format (.slnx) or text format (.sln)
        if content.trim_start().starts_with("<?xml") || content.trim_start().starts_with("<Solution") {
            Self::parse_slnx(content, base_dir)
        } else {
            Self::parse_sln(content, base_dir)
        }
    }

    /// Parse a traditional .sln file (text format)
    fn parse_sln(content: &str, base_dir: &Path) -> Result<Self> {
        let mut projects = Vec::new();
        let mut configurations = Vec::new();
        let mut startup_project = None;

        for line in content.lines() {
            let line = line.trim();

            // Parse project entries: Project("{type-guid}") = "name", "path", "{guid}"
            if line.starts_with("Project(\"") {
                if let Some(project) = parse_project_line(line) {
                    projects.push(project);
                }
            }

            // Parse solution configurations
            if line.starts_with("Debug|") || line.starts_with("Release|") {
                if let Some(config) = line.split('|').next() {
                    if !configurations.contains(&config.to_string()) {
                        configurations.push(config.to_string());
                    }
                }
            }

            // Parse startup project configuration
            if line.contains("StartupProject") {
                if let Some(guid) = extract_guid(line) {
                    startup_project = Some(guid);
                }
            }
        }

        // Default to first executable project if no startup project specified
        if startup_project.is_none() && !projects.is_empty() {
            startup_project = Some(projects[0].guid.clone());
        }

        Ok(SolutionFile {
            path: base_dir.join("solution.sln"),
            projects,
            configurations: if configurations.is_empty() {
                vec!["Debug".to_string(), "Release".to_string()]
            } else {
                configurations
            },
            startup_project,
        })
    }

    /// Parse a .slnx file (XML format)
    fn parse_slnx(content: &str, base_dir: &Path) -> Result<Self> {
        let mut projects = Vec::new();
        let mut startup_project = None;

        // Simple XML parsing for .slnx format
        // .slnx format structure:
        // <Solution>
        //   <Projects>
        //     <Project Path="..." />
        //   </Projects>
        // </Solution>
        
        // Extract projects - look for <Project> tags
        let mut remaining = content;
        while let Some(project_start) = remaining.find("<Project") {
            let project_end = remaining[project_start..].find(">").ok_or_else(|| {
                anyhow::anyhow!("Invalid XML: unclosed Project tag")
            })?;
            
            let project_tag = &remaining[project_start..project_start + project_end + 1];
            
            // Extract Path attribute
            if let Some(path_start) = project_tag.find("Path=\"") {
                let path_start = path_start + 6; // Skip "Path=\""
                if let Some(path_end) = project_tag[path_start..].find('"') {
                    let path_str = &project_tag[path_start..path_start + path_end];
                    let path = PathBuf::from(path_str);
                    
                    // Extract name from path (filename without extension)
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    
                    // Generate a simple GUID for .slnx projects (we'll use a hash of the path)
                    let mut hasher = DefaultHasher::new();
                    path_str.hash(&mut hasher);
                    let hash = hasher.finish();
                    // Format as GUID: {8 hex}-{4 hex}-{4 hex}-{4 hex}-{12 hex}
                    let guid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", 
                        (hash >> 32) as u32, 
                        ((hash >> 16) & 0xFFFF) as u16, 
                        (hash & 0xFFFF) as u16,
                        ((hash >> 48) & 0xFFFF) as u16,
                        hash & 0xFFFFFFFFFFFF);
                    
                    projects.push(SolutionProject {
                        name,
                        path,
                        guid,
                        type_guid: "FAE04EC0-301F-11D3-BA7A-00C04FC2CCAE".to_string(), // C# project type GUID
                        packages: Vec::new(), // Packages will be loaded separately
                    });
                }
            }
            
            remaining = &remaining[project_start + project_end + 1..];
        }

        // Default to first executable project if no startup project specified
        if startup_project.is_none() && !projects.is_empty() {
            startup_project = Some(projects[0].guid.clone());
        }

        Ok(SolutionFile {
            path: base_dir.join("solution.slnx"),
            projects,
            configurations: vec!["Debug".to_string(), "Release".to_string()],
            startup_project,
        })
    }

    /// Get a project by name
    pub fn get_project(&self, name: &str) -> Option<&SolutionProject> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Get a project by GUID
    pub fn get_project_by_guid(&self, guid: &str) -> Option<&SolutionProject> {
        self.projects.iter().find(|p| p.guid == guid)
    }

    /// Get the startup project
    pub fn get_startup_project(&self) -> Option<&SolutionProject> {
        self.startup_project
            .as_ref()
            .and_then(|guid| self.get_project_by_guid(guid))
    }

    /// Get all executable projects (likely to have a Main entry point)
    pub fn get_executable_projects(&self) -> Vec<&SolutionProject> {
        // Heuristic: projects with names not ending in "Tests" or containing "Test"
        self.projects
            .iter()
            .filter(|p| !p.name.contains("Test"))
            .collect()
    }
}

/// Parse a project line from a .sln file
/// Format: Project("{type-guid}") = "name", "path", "{guid}"
fn parse_project_line(line: &str) -> Option<SolutionProject> {
    // Extract type GUID
    let type_guid = extract_guid(line)?;

    // Extract project name and path
    if let Some(after_equals) = line.split('=').nth(1) {
        let parts: Vec<&str> = after_equals
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();

        if parts.len() >= 3 {
            let name = parts[0].to_string();
            let path = PathBuf::from(parts[1]);
            let guid = parts[2].to_string();

            return Some(SolutionProject {
                name,
                path,
                guid,
                type_guid,
                packages: Vec::new(), // Packages will be loaded separately
            });
        }
    }

    None
}

/// Extract a GUID from a string in the format {GUID}
fn extract_guid(line: &str) -> Option<String> {
    if let Some(start) = line.find('{') {
        if let Some(end) = line[start..].find('}') {
            let guid = &line[start + 1..start + end];
            return Some(guid.to_string());
        }
    }
    None
}

/// Parse a .csproj file to extract NuGet package references
pub fn parse_csproj_packages(content: &str) -> Result<Vec<NuGetPackage>> {
    let mut packages = Vec::new();
    
    // Simple XML parsing for PackageReference items
    // Format: <PackageReference Include="PackageId" Version="1.0.0" />
    // or: <PackageReference Include="PackageId" />
    
    let mut remaining = content;
    while let Some(ref_start) = remaining.find("<PackageReference") {
        let ref_end = remaining[ref_start..].find("/>")
            .or_else(|| remaining[ref_start..].find("</PackageReference>"))
            .ok_or_else(|| anyhow::anyhow!("Invalid XML: unclosed PackageReference tag"))?;
        
        let ref_tag = &remaining[ref_start..ref_start + ref_end + 2];
        
        // Extract Include attribute (package ID)
        let package_id = if let Some(include_start) = ref_tag.find("Include=\"") {
            let include_start = include_start + 9; // Skip "Include=\""
            if let Some(include_end) = ref_tag[include_start..].find('"') {
                Some(ref_tag[include_start..include_start + include_end].to_string())
            } else {
                None
            }
        } else {
            None
        };
        
        // Extract Version attribute (optional)
        let package_version = if let Some(version_start) = ref_tag.find("Version=\"") {
            let version_start = version_start + 9; // Skip "Version=\""
            if let Some(version_end) = ref_tag[version_start..].find('"') {
                Some(ref_tag[version_start..version_start + version_end].to_string())
            } else {
                None
            }
        } else {
            None
        };
        
        if let Some(id) = package_id {
            packages.push(NuGetPackage {
                id,
                version: package_version,
            });
        }
        
        remaining = &remaining[ref_start + ref_end + 2..];
    }
    
    Ok(packages)
}
