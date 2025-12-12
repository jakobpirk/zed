# .NET Debugging Implementation - Testing Guide

## Implementation Summary

This implementation adds comprehensive .NET debugging support to Zed, including:

1. **Solution and Project File Detection** - Automatic recognition of .csproj and .sln files
2. **Build Task Integration** - Task templates for dotnet build, run, test, and clean commands
3. **Debug Adapter Integration** - vsdbg (Microsoft's official .NET debugger) support
4. **Debug Workflow** - Automatic conversion of dotnet run tasks to debug sessions with breakpoint support

## Files Created

### Core Implementation Files

1. **crates/languages/src/csharp.rs** - NEW
   - `CsprojManifestProvider`: Detects .csproj files
   - `SolutionManifestProvider`: Detects .sln files
   - `CSharpContextProvider`: Provides task templates (build, run, test, clean)

2. **crates/dap_adapters/src/dotnet.rs** - NEW
   - `DotNetDebugAdapter`: Implements vsdbg debug adapter
   - Handles vsdbg binary discovery in PATH or cache
   - Provides DAP schema for launch/attach configurations

3. **crates/project/src/debugger/locators/dotnet.rs** - NEW
   - `DotNetLocator`: Converts dotnet tasks to debug scenarios
   - Parses build output to find compiled DLL
   - Handles multiple .NET versions and output paths

### Files Modified

- `crates/languages/src/lib.rs` - Added manifest provider registrations
- `crates/dap_adapters/src/dap_adapters.rs` - Added DotNetDebugAdapter registration
- `crates/project/src/debugger/locators.rs` - Added dotnet module declaration
- `crates/project/src/debugger/dap_store.rs` - Added DotNetLocator registration

## Testing Prerequisites

### System Requirements

1. **.NET SDK Installed**
   ```bash
   dotnet --version  # Should show .NET 6.0 or higher
   ```

2. **vsdbg Installed (Optional - will be auto-detected)**
   - If vsdbg is in system PATH, it will be used automatically
   - Otherwise, download from: https://github.com/microsoft/vscode-csharp
   - Or install via: `dotnet tool install --global vsdbg`

3. **C# Extension Installed**
   - Zed should have the C# extension enabled for syntax highlighting and IntelliSense

## Testing Procedure

### Step 1: Create a Test .NET Application

```bash
# Create a test directory
mkdir -p ~/zed-test/ConsoleApp
cd ~/zed-test/ConsoleApp

# Create a new console application
dotnet new console

# Build to ensure it compiles
dotnet build
```

### Step 2: Open Project in Zed

```bash
# Open the project directory
zed .

# Or open the .csproj file directly
zed ConsoleApp.csproj
```

### Step 3: Verify Project Detection

1. **Open the project file** (ConsoleApp.csproj or .sln)
2. **Check that Zed recognizes it as a .NET project**
   - Look for syntax highlighting (provided by C# extension)
   - Check that IntelliSense works (from OmniSharp LSP)

### Step 4: Test Build Tasks

1. **Open the Command Palette** (Ctrl+Shift+P / Cmd+Shift+P)
2. **Type "task"** and select "Tasks: Run Task"
3. **Verify these tasks appear**:
   - dotnet: build
   - dotnet: run
   - dotnet: test
   - dotnet: clean

4. **Run "dotnet: build"**
   - Should compile successfully
   - Output should show compilation messages

### Step 5: Test Debug Workflow

#### Using Manual Task → Debug

1. **Modify Program.cs** to add some lines:
   ```csharp
   Console.WriteLine("Starting application");
   int x = 5;
   int y = 10;
   Console.WriteLine($"Sum: {x + y}");
   ```

2. **Set a breakpoint**:
   - Click on the line number in the editor where you want to break
   - A red circle should appear indicating the breakpoint

3. **Run Debug Task**:
   - Open Tasks panel (Ctrl+Shift+B / Cmd+Shift+B)
   - Select "dotnet: run"
   - Click the Debug button (should appear next to the task)

4. **Expected behavior**:
   - vsdbg should launch
   - Execution should pause at your breakpoint
   - You should be able to inspect variables in the Debug panel

#### Debug Actions to Test

Once debugging is active:

1. **Step Over** (F10)
   - Should move to next line in current function

2. **Step Into** (F11)
   - Should move into function calls

3. **Step Out** (Shift+F11)
   - Should move back to caller

4. **Continue** (F5)
   - Should resume execution to next breakpoint or end

5. **Variable Inspection**
   - Hover over variables to see their values
   - Check the Variables panel in the Debug view

6. **Call Stack**
   - Verify call stack shows correct function nesting

### Step 6: Test with Multi-Project Solution

1. **Create a solution with multiple projects**:
   ```bash
   mkdir -p ~/zed-test/MultiProject
   cd ~/zed-test/MultiProject

   # Create solution
   dotnet new sln

   # Create projects
   dotnet new console -n ConsoleApp
   dotnet new classlib -n LibraryProject

   # Add projects to solution
   dotnet sln add ConsoleApp/ConsoleApp.csproj
   dotnet sln add LibraryProject/LibraryProject.csproj
   ```

2. **Add reference from ConsoleApp to LibraryProject**:
   ```bash
   cd ConsoleApp
   dotnet add reference ../LibraryProject/LibraryProject.csproj
   ```

3. **Open solution in Zed**:
   ```bash
   zed ..
   ```

4. **Verify**:
   - Opening the .sln file shows it's recognized
   - Tasks still appear for building the solution
   - Debugging works for projects in the solution

## Troubleshooting

### Issue: vsdbg not found

**Solution**:
```bash
# Option 1: Install .NET SDK (includes vsdbg)
dotnet --version

# Option 2: Install vsdbg directly
dotnet tool install --global vsdbg

# Verify installation
which vsdbg  # Linux/Mac
where vsdbg.exe  # Windows
```

### Issue: No debug option appears in tasks

**Check**:
1. Verify dotnet command is in PATH: `which dotnet` or `where dotnet.exe`
2. Verify the .csproj file exists in project
3. Check that you're running a "dotnet: run" or "dotnet: build" task
4. Verify vsdbg is installed or in PATH

### Issue: Breakpoints not working

**Check**:
1. Ensure the project was built in Debug configuration: `dotnet build --configuration Debug`
2. Verify the C# extension is installed and working
3. Check debug adapter output for errors

### Issue: Variables show as <unavailable>

**Possible causes**:
1. Debug configuration is Release (need Debug)
2. Optimizations enabled (Debugs should have optimization disabled)
3. vsdbg version incompatibility

**Solution**:
```bash
# Rebuild with Debug configuration
dotnet clean
dotnet build --configuration Debug
```

## Advanced Testing

### Test Different Project Types

1. **Console Application**
   ```bash
   dotnet new console -n ConsoleApp
   ```

2. **ASP.NET Web App** (if needed)
   ```bash
   dotnet new webapp -n WebApp
   ```

3. **Class Library**
   ```bash
   dotnet new classlib -n ClassLib
   ```

4. **Unit Test Project**
   ```bash
   dotnet new xunit -n UnitTests
   ```

### Test Different .NET Versions

Create projects targeting different frameworks:
```bash
# .NET 8.0
dotnet new console --framework net8.0

# .NET 6.0
dotnet new console --framework net6.0

# .NET Framework (if on Windows)
dotnet new console --framework net481
```

### Performance Testing

1. **Debug large application** - Verify debugging is responsive
2. **Debug with many breakpoints** - Test breakpoint handling
3. **Step through large functions** - Verify stepping performance

## Success Criteria

All of the following should work:

- ✅ Opening .csproj files shows C# syntax highlighting
- ✅ IntelliSense/completions work in C# files
- ✅ "Tasks: Run Task" shows dotnet: build, run, test, clean
- ✅ Building projects completes successfully
- ✅ Setting breakpoints shows visual indicator
- ✅ Running debug task launches vsdbg
- ✅ Execution pauses at breakpoints
- ✅ Can inspect variable values
- ✅ Step over/into/out works correctly
- ✅ Call stack displays correctly
- ✅ Works with multi-project solutions

## Reporting Issues

If you encounter issues:

1. **Check the debug console output** for error messages
2. **Verify all prerequisites** are installed
3. **Try with a fresh .NET project** to isolate issues
4. **Check vsdbg compatibility** with your .NET version
5. **Report to Zed GitHub Issues** with:
   - Zed version
   - .NET SDK version
   - vsdbg version
   - Steps to reproduce
   - Error messages or logs

## Next Steps After Testing

Once basic debugging is working, consider:

1. **Improve solution file support** - Parse .sln to extract project structure
2. **Multi-project debugging** - Select startup project
3. **ASP.NET debugging** - Browser launch and web app debugging
4. **Custom debug configurations** - .zed/debug.json support
5. **Test result integration** - Show test results in editor

## References

- [vsdbg Repository](https://github.com/microsoft/vscode-csharp/tree/main/debugAdapters)
- [Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/)
- [.NET SDK Documentation](https://docs.microsoft.com/dotnet/)
- [Zed Debug System Documentation](https://zed.dev/docs/system)
