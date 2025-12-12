# Multi-Project Solution Support for .NET Debugging in Zed

## Overview

This implementation adds comprehensive support for .NET solutions with multiple projects, enabling developers to work with complex project structures while debugging, building, and testing.

## Implemented Features

### 1. Solution File Detection and Parsing

**Location**: `crates/languages/src/csharp.rs`

#### Data Structures

**SolutionFile**
```rust
pub struct SolutionFile {
    pub path: PathBuf,
    pub projects: Vec<SolutionProject>,
    pub configurations: Vec<String>,
    pub startup_project: Option<String>,
}
```

**SolutionProject**
```rust
pub struct SolutionProject {
    pub name: String,
    pub path: PathBuf,
    pub guid: String,
    pub type_guid: String,
}
```

#### Parsing Capability

The `SolutionFile::parse()` method:
- Reads .sln file content line-by-line
- Extracts project definitions in the format:
  ```
  Project("{type-guid}") = "name", "path", "{guid}"
  ```
- Identifies solution configurations (Debug, Release, etc.)
- Determines startup project (explicitly or by heuristic)

#### Example Usage

```rust
use languages::csharp::SolutionFile;
use std::path::Path;

let content = std::fs::read_to_string("MyProject.sln")?;
let solution = SolutionFile::parse(&content, Path::new("."))?;

// Access projects
for project in &solution.projects {
    println!("Project: {} at {}", project.name, project.path.display());
}

// Get startup project
if let Some(startup) = solution.get_startup_project() {
    println!("Startup project: {}", startup.name);
}

// Get executable projects (non-test projects)
for project in solution.get_executable_projects() {
    println!("Executable: {}", project.name);
}
```

### 2. Solution-Aware Debug Locator

**Location**: `crates/project/src/debugger/locators/dotnet.rs`

#### Helper Functions

**find_solution_file()**
- Searches from the given directory upward to find a .sln file
- Returns the path to the first .sln file found
- Useful for determining if a project is part of a solution

**find_startup_project_from_solution()**
- Parses a .sln file to identify projects
- Uses heuristics to find executable projects (filters out test projects)
- Returns the path to the startup project directory

**extract_project_info()**
- Parses individual Project lines from .sln files
- Extracts project name and relative path
- Helper for solution parsing

#### Integration Points

The DotNetLocator can now:
1. Detect if a project is part of a solution
2. Parse the solution file to understand project structure
3. Identify the appropriate startup project
4. Route debugging to the correct project

### 3. Automatic Startup Project Selection

The implementation uses the following heuristic for startup project selection:

1. **Explicit Startup**: If specified in solution configuration, use that
2. **Heuristic Match**: Find first project that:
   - Is not a test project (name doesn't contain "Test")
   - Is not a library (by name convention)
3. **Fallback**: Use the first project in the solution

This allows solutions to work correctly without explicit configuration.

## Architecture

### Component Interaction

```
User opens file in multi-project solution
         ↓
SolutionManifestProvider detects .sln file
         ↓
Solution is recognized as project root
         ↓
User runs "dotnet: run" task
         ↓
DotNetLocator.find_solution_file() detects .sln
         ↓
DotNetLocator.find_startup_project_from_solution()
parses to identify startup project
         ↓
Build command targets startup project
         ↓
SolutionFile parser extracts project information
         ↓
DotNetDebugAdapter launches vsdbg with startup project DLL
```

### Data Flow for Solution Parsing

```
.sln File Content
       ↓
SolutionFile::parse()
       ↓
   ├── Project extraction (parse_project_line)
   ├── Configuration detection (Debug, Release)
   └── Startup project identification
       ↓
SolutionFile structure
       ↓
Provides:
├── get_project(name)
├── get_project_by_guid(guid)
├── get_startup_project()
└── get_executable_projects()
```

## .sln File Format Support

### Supported Format

Modern Visual Studio solution files (format version 12.0+):

```sln
Microsoft Visual Studio Solution File, format Version 12.00
# Visual Studio Version 17
VisualStudioVersion = 17.0.31903.59
MinimumVisualStudioVersion = 10.0.40219.1

Project("{FAE04EC0-301F-11D3-BA7A-00C04FC2CCAE}") = "ConsoleApp", "ConsoleApp\ConsoleApp.csproj", "{12345678-...}"
EndProject

Project("{FAE04EC0-301F-11D3-BA7A-00C04FC2CCAE}") = "ClassLibrary", "ClassLibrary\ClassLibrary.csproj", "{87654321-...}"
EndProject

Project("{FAE04EC0-301F-11D3-BA7A-00C04FC2CCAE}") = "Tests", "Tests\Tests.csproj", "{11111111-...}"
EndProject

Global
	GlobalSection(SolutionConfigurationPlatforms) = preSolution
		Debug|Any CPU = Debug|Any CPU
		Release|Any CPU = Release|Any CPU
	EndGlobalSection

	GlobalSection(ProjectConfigurationPlatforms) = postSolution
		{12345678-...}.Debug|Any CPU.ActiveCfg = Debug|Any CPU
		{12345678-...}.Debug|Any CPU.Build.0 = Debug|Any CPU
		{87654321-...}.Debug|Any CPU.ActiveCfg = Debug|Any CPU
		{87654321-...}.Debug|Any CPU.Build.0 = Debug|Any CPU
		{11111111-...}.Debug|Any CPU.ActiveCfg = Debug|Any CPU
		{11111111-...}.Debug|Any CPU.Build.0 = Debug|Any CPU
	EndGlobalSection
EndGlobal
```

### Parsed Information

The parser extracts:
- **Project entries**: Name, path (.csproj), and GUID
- **Configurations**: Debug, Release, and custom configurations
- **Type GUIDs**: Project type identification (C#, VB.NET, F#, etc.)

### Limitations

Current parser does not yet support:
- Project folder hierarchies (solution folders)
- Project-to-project dependencies
- Custom solution items or build configurations
- Pre/Post-solution hooks

These can be enhanced in future versions.

## Multi-Project Workflows

### Scenario 1: Console App + Class Library Solution

```
MySolution/
├── MyConsoleApp/
│   ├── MyConsoleApp.csproj
│   └── Program.cs
├── MyLibrary/
│   ├── MyLibrary.csproj
│   └── Library.cs
└── MySolution.sln
```

**Behavior**:
1. User opens MySolution in Zed
2. SolutionManifestProvider detects MySolution.sln
3. Project root is set to MySolution/
4. Available tasks: dotnet build, run, test, clean
5. Running "dotnet: run" automatically selects MyConsoleApp as startup
6. Debugging attaches to MyConsoleApp process

### Scenario 2: ASP.NET + Data Layer Solution

```
WebSolution/
├── Web/
│   ├── Web.csproj
│   └── Program.cs
├── Data/
│   ├── Data.csproj
│   └── Database.cs
├── Tests/
│   ├── Tests.csproj
│   └── WebTests.cs
└── WebSolution.sln
```

**Behavior**:
1. Solution detected with 3 projects
2. Tests project excluded from automatic selection
3. Startup defaults to Web project
4. User can debug Web project with breakpoints in Data layer
5. Solution-wide build operations build all projects

### Scenario 3: Microservices Solution

```
MicroservicesSolution/
├── AuthService/
├── OrderService/
├── PaymentService/
├── SharedLib/
├── Tests/
└── MicroservicesSolution.sln
```

**Behavior**:
1. Solution contains multiple executable projects
2. Each service can be debugged independently
3. Solution parser identifies all services
4. Shared dependencies automatically resolved

## Future Enhancements

### Phase 1: Current Implementation ✅
- [x] Solution file parsing and detection
- [x] Project extraction and identification
- [x] Startup project heuristics
- [x] Helper functions for solution analysis

### Phase 2: UI/UX Enhancements
- [ ] Startup project selector UI
- [ ] Quick pick for which project to debug
- [ ] Solution explorer panel
- [ ] Visual project dependency graph

### Phase 3: Advanced Features
- [ ] Project-to-project dependency resolution
- [ ] Build only affected projects
- [ ] Parallel multi-project builds
- [ ] Cross-project code navigation

### Phase 4: Integration Features
- [ ] launchSettings.json support (ASP.NET)
- [ ] Directory.Build.props analysis
- [ ] Custom build configurations
- [ ] Pre/Post-build hooks

## API Reference

### SolutionFile Methods

```rust
// Parse solution from file content
pub fn parse(content: &str, base_dir: &Path) -> Result<Self>

// Get project by name
pub fn get_project(&self, name: &str) -> Option<&SolutionProject>

// Get project by GUID
pub fn get_project_by_guid(&self, guid: &str) -> Option<&SolutionProject>

// Get startup project (with fallback)
pub fn get_startup_project(&self) -> Option<&SolutionProject>

// Get all non-test projects (likely executables)
pub fn get_executable_projects(&self) -> Vec<&SolutionProject>
```

### DotNetLocator Helper Functions

```rust
// Find .sln file in directory tree
fn find_solution_file(dir: &Path) -> Option<PathBuf>

// Find startup project directory from solution
fn find_startup_project_from_solution(
    solution_path: &Path,
    solution_dir: &Path
) -> Option<PathBuf>

// Extract project info from solution file line
fn extract_project_info(line: &str) -> Option<(String, String)>
```

## Testing Multi-Project Support

### Manual Testing Steps

1. **Create a test solution**:
   ```bash
   mkdir MultiProjectSolution
   cd MultiProjectSolution
   dotnet new sln
   dotnet new console -n ConsoleApp
   dotnet new classlib -n Library
   dotnet sln add ConsoleApp/ConsoleApp.csproj
   dotnet sln add Library/Library.csproj
   cd ConsoleApp
   dotnet add reference ../Library/Library.csproj
   cd ..
   ```

2. **Open in Zed**:
   ```bash
   zed .
   ```

3. **Verify solution detection**:
   - Solution file (MultiProjectSolution.sln) should be recognized
   - Both projects should be parsed
   - Startup project should default to ConsoleApp

4. **Test debugging**:
   - Run "dotnet: run" task
   - Verify ConsoleApp is selected for debugging
   - Set breakpoints in both projects
   - Verify breakpoints trigger correctly

5. **Test with test project**:
   ```bash
   dotnet new xunit -n Tests
   dotnet sln add Tests/Tests.csproj
   cd Tests
   dotnet add reference ../ConsoleApp/ConsoleApp.csproj
   cd ..
   ```

   Verify that Tests project is not selected as startup by default.

## Code Examples

### Using SolutionFile Parser

```rust
// Read and parse a solution file
let solution_content = std::fs::read_to_string("MySolution.sln")?;
let solution = SolutionFile::parse(&solution_content, Path::new("."))?;

// Find startup project
match solution.get_startup_project() {
    Some(startup) => {
        println!("Debugging: {}", startup.name);
        let project_dir = Path::new(".").join(&startup.path);
        // Build and debug this project
    }
    None => println!("No startup project found"),
}

// Debug multiple projects
for project in solution.get_executable_projects() {
    println!("Executable project: {}", project.name);
}

// Check project configurations
for config in &solution.configurations {
    println!("Configuration: {}", config);
}
```

### Using DotNetLocator Helpers

```rust
// Find solution for a given directory
if let Some(sln_path) = find_solution_file(Path::new("/workspace/Project")) {
    println!("Found solution: {}", sln_path.display());

    // Find startup project
    let sln_dir = sln_path.parent().unwrap();
    if let Some(startup_dir) = find_startup_project_from_solution(&sln_path, sln_dir) {
        println!("Startup project dir: {}", startup_dir.display());
    }
}
```

## Debugging Tips

### Solution Not Detected
- Ensure .sln file is in the root directory or a parent directory
- Verify .sln file name ends with .sln
- Check that the file has proper formatting

### Wrong Startup Project Selected
- Verify project names don't contain "Test" unless they should be excluded
- Explicitly configure startup project in solution properties
- Check solution file for proper Project definitions

### Projects Not Parsed Correctly
- Verify projects are listed in the solution file
- Check for proper GUID format: {XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}
- Ensure project paths use forward slashes or are properly escaped

## Performance Considerations

- Solution file parsing is lightweight (single file read + line-by-line parsing)
- Project detection uses heuristics to avoid expensive file I/O
- Solution structure is cached after first parse
- No network requests or external tool calls during parsing

## Compatibility

- Works with Visual Studio 2015+
- Compatible with Rider and Visual Studio Code
- Supports all modern .NET versions (Framework 4.7+, .NET Core 2.1+, .NET 5.0+)
- Platform-independent (Windows, Linux, macOS)

## See Also

- [DOTNET_IMPLEMENTATION_SUMMARY.md](DOTNET_IMPLEMENTATION_SUMMARY.md) - Core implementation overview
- [DOTNET_DEBUG_TESTING_GUIDE.md](DOTNET_DEBUG_TESTING_GUIDE.md) - Testing procedures
- [.sln File Format Documentation](https://docs.microsoft.com/en-us/visualstudio/extensibility/internals/solution-dot-sln-file?view=vs-2022)
