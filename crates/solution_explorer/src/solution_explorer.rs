use anyhow::Result;
use collections::HashSet;
use gpui::{
    Action, App, AsyncWindowContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, Point, Pixels, Render, Subscription, Task, UniformListScrollHandle,
    WeakEntity, Window, actions, anchored, deferred, div, px, uniform_list,
};
use languages::csharp::{SolutionFile, parse_csproj_packages};
use project::{Fs, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{DockSide, Settings, SettingsStore, update_settings_file};
use std::path::{Path, PathBuf};
use ui::{
    Color, ContextMenu, Icon, IconName, Label, LabelSize, ListItem, ListItemSpacing, ScrollAxes,
    Scrollbars, WithScrollbar, prelude::*,
};
use workspace::{
    OpenOptions, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};
use zed_actions::{solution_explorer::ToggleFocus, task::Spawn};
use task::SpawnInTerminal;

const SOLUTION_EXPLORER_PANEL_KEY: &str = "SolutionExplorerPanel";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SolutionExplorerSettings {
    #[serde(default)]
    pub dock: Option<DockSide>,
    #[serde(default = "default_width")]
    pub default_width: Pixels,
    #[serde(default)]
    pub starts_open: bool,
    /// Directory to copy .nupkg files to after packing
    #[serde(default)]
    pub pack_output_directory: Option<String>,
}

fn default_width() -> Pixels {
    px(250.0)
}

impl Default for SolutionExplorerSettings {
    fn default() -> Self {
        Self {
            dock: Some(DockSide::Left),
            default_width: default_width(),
            starts_open: false,
        }
    }
}

impl Settings for SolutionExplorerSettings {
    const KEY: Option<&'static str> = Some("solution_explorer");

    fn file_name() -> &'static str {
        "solution_explorer"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SolutionTreeNode {
    Solution { path: PathBuf },
    Project { name: String, path: PathBuf, guid: String },
    Package { project_guid: String, package_id: String, version: Option<String> },
}

struct SolutionTreeState {
    solution: Option<SolutionFile>,
    expanded_projects: HashSet<String>, // Project GUIDs
    expanded_packages: HashSet<String>, // Project GUIDs that have packages expanded
    selected_nodes: HashSet<SolutionTreeNode>, // Support multi-selection
}

impl Default for SolutionTreeState {
    fn default() -> Self {
        Self {
            solution: None,
            expanded_projects: HashSet::new(),
            expanded_packages: HashSet::new(),
            selected_nodes: HashSet::new(),
        }
    }
}

pub struct SolutionExplorerPanel {
    project: gpui::Entity<Project>,
    fs: Arc<dyn Fs>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    width: Option<Pixels>,
    state: SolutionTreeState,
    solution_load_task: Task<()>,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
}

actions!(
    solution_explorer,
    [
        /// Toggles focus on the solution explorer panel.
        ToggleFocus,
        /// Expands the selected project in the solution tree.
        ExpandSelectedProject,
        /// Collapses the selected project in the solution tree.
        CollapseSelectedProject,
        /// Expands all projects in the solution tree.
        ExpandAllProjects,
        /// Collapses all projects in the solution tree.
        CollapseAllProjects,
        /// Opens the selected project file.
        OpenSelectedProject,
    ]
);

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct BuildSolution;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct RebuildSolution;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct CleanSolution;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct OpenSolutionFile;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct BuildProject {
    pub project_name: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct RebuildProject {
    pub project_name: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct CleanProject {
    pub project_name: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct SetStartupProject {
    pub project_guid: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct UnsetStartupProject;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct OpenProjectFile {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, Action)]
#[action(namespace = solution_explorer)]
pub struct OpenProjectFolder {
    pub path: PathBuf,
}

impl SolutionExplorerPanel {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        project: gpui::Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fs = project.read(cx).fs().clone();
        let focus_handle = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::default();

        let mut panel = Self {
            project,
            fs,
            workspace,
            focus_handle,
            scroll_handle,
            width: None,
            state: SolutionTreeState::default(),
            solution_load_task: Task::ready(()),
            context_menu: None,
        };

        panel.load_solution(window, cx);
        panel
    }

    fn load_solution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let fs = self.fs.clone();
        let panel_entity = window.entity();

        self.solution_load_task = cx.spawn(|mut cx| async move {
            let solution_path = project
                .update(&mut cx, |project, cx| {
                    // Find solution file in project
                    project
                        .worktrees()
                        .find_map(|worktree| {
                            let root = worktree.read(cx).abs_path();
                            find_solution_file(&root, &fs)
                        })
                })
                .ok()
                .flatten();

            if let Some(solution_path) = solution_path {
                if let Ok(content) = std::fs::read_to_string(&solution_path) {
                    let base_dir = solution_path.parent().unwrap_or(Path::new("."));
                    if let Ok(mut solution) = SolutionFile::parse(&content, base_dir) {
                        // Load packages for each project
                        for project in &mut solution.projects {
                            let project_path = base_dir.join(&project.path);
                            if let Ok(csproj_content) = std::fs::read_to_string(&project_path) {
                                if let Ok(packages) = parse_csproj_packages(&csproj_content) {
                                    project.packages = packages;
                                }
                            }
                        }
                        
                        panel_entity.update(&mut cx, |panel, cx| {
                            panel.state.solution = Some(solution);
                            cx.notify();
                        })
                        .ok();
                    }
                }
            }
        });
    }

    fn find_solution_file(&self, cx: &mut Context<Self>) -> Option<PathBuf> {
        self.project
            .read(cx)
            .worktrees()
            .find_map(|worktree| {
                let root = worktree.read(cx).abs_path();
                find_solution_file(&root, &self.fs)
            })
    }

    fn render_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<_> = if let Some(ref solution) = self.state.solution {
            let mut items = Vec::new();
            let expanded_projects = self.state.expanded_projects.clone();
            let selected_nodes = self.state.selected_nodes.clone();

            // Solution root node
            items.push(TreeItem {
                node: SolutionTreeNode::Solution {
                    path: solution.path.clone(),
                },
                label: solution
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Solution")
                    .to_string(),
                icon: Some(IconName::FileCode),
                depth: 0,
                is_expanded: true,
                has_children: !solution.projects.is_empty(),
            });

            // Project nodes
            let expanded_packages = self.state.expanded_packages.clone();
            for project in &solution.projects {
                let is_expanded = expanded_projects.contains(&project.guid);
                let packages_expanded = expanded_packages.contains(&project.guid);
                items.push(TreeItem {
                    node: SolutionTreeNode::Project {
                        name: project.name.clone(),
                        path: project.path.clone(),
                        guid: project.guid.clone(),
                    },
                    label: project.name.clone(),
                    icon: Some(IconName::FileCode),
                    depth: 1,
                    is_expanded,
                    has_children: !project.packages.is_empty(),
                });
                
                // Add package nodes if project is expanded and packages are expanded
                if is_expanded && packages_expanded {
                    for package in &project.packages {
                        items.push(TreeItem {
                            node: SolutionTreeNode::Package {
                                project_guid: project.guid.clone(),
                                package_id: package.id.clone(),
                                version: package.version.clone(),
                            },
                            label: if let Some(ref version) = package.version {
                                format!("{} ({})", package.id, version)
                            } else {
                                package.id.clone()
                            },
                            icon: Some(IconName::Box),
                            depth: 2,
                            is_expanded: false,
                            has_children: false,
                        });
                    }
                }
            }

            items
        } else {
            vec![]
        };

        let item_count = items.len();
        let expanded_projects = self.state.expanded_projects.clone();
        let expanded_packages = self.state.expanded_packages.clone();
        let selected_nodes = self.state.selected_nodes.clone();

        uniform_list(
            cx,
            "solution_explorer_tree",
            item_count,
            move |cx, range, _| {
                items[range]
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let is_selected = selected_nodes.contains(&item.node);
                        let (item_guid, is_package_node) = match &item.node {
                            SolutionTreeNode::Project { guid, .. } => (Some(guid.clone()), false),
                            SolutionTreeNode::Package { project_guid, .. } => (Some(project_guid.clone()), true),
                            _ => (None, false),
                        };
                        let is_expanded = item_guid
                            .as_ref()
                            .map(|g| expanded_projects.contains(g))
                            .unwrap_or(false);
                        let packages_expanded = item_guid
                            .as_ref()
                            .map(|g| expanded_packages.contains(g))
                            .unwrap_or(false);

                        ListItem::new(index)
                            .spacing(ListItemSpacing::Sparse)
                            .selected(is_selected)
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                if event.is_right_click() {
                                    this.deploy_context_menu(event.position, &item.node, window, cx);
                                    return;
                                }
                                // Toggle selection: if Ctrl/Cmd is held, add/remove from selection; otherwise, replace selection
                                if event.modifiers.control || event.modifiers.command {
                                    if this.state.selected_nodes.contains(&item.node) {
                                        this.state.selected_nodes.remove(&item.node);
                                    } else {
                                        this.state.selected_nodes.insert(item.node.clone());
                                    }
                                } else {
                                    // Single selection - clear and set
                                    this.state.selected_nodes.clear();
                                    this.state.selected_nodes.insert(item.node.clone());
                                }
                                match &item.node {
                                    SolutionTreeNode::Project { guid, .. } => {
                                        // Toggle project expansion
                                        if this.state.expanded_projects.contains(guid) {
                                            this.state.expanded_projects.remove(guid);
                                            this.state.expanded_packages.remove(guid);
                                        } else {
                                            this.state.expanded_projects.insert(guid.clone());
                                            // Auto-expand packages if project has packages
                                            if let Some(ref solution) = this.state.solution {
                                                if let Some(proj) = solution.projects.iter().find(|p| p.guid == *guid) {
                                                    if !proj.packages.is_empty() {
                                                        this.state.expanded_packages.insert(guid.clone());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    SolutionTreeNode::Package { project_guid, .. } => {
                                        // Toggle package expansion for the parent project
                                        if this.state.expanded_packages.contains(project_guid) {
                                            this.state.expanded_packages.remove(project_guid);
                                        } else {
                                            this.state.expanded_packages.insert(project_guid.clone());
                                        }
                                    }
                                    _ => {}
                                }
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .pl(px(item.depth as f32 * 16.0))
                                    .when(item.has_children, |div| {
                                        let chevron_expanded = match &item.node {
                                            SolutionTreeNode::Project { .. } => is_expanded && packages_expanded,
                                            _ => is_expanded,
                                        };
                                        div.child(
                                            Icon::new(if chevron_expanded {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .size(ui::IconSize::Small)
                                            .color(Color::Muted),
                                        )
                                    })
                                    .when(!item.has_children, |div| div.w(px(16.0)))
                                    .when_some(item.icon.clone(), |div, icon| {
                                        div.child(Icon::new(icon).size(ui::IconSize::Small))
                                    })
                                    .child(Label::new(item.label.clone()).size(LabelSize::Small)),
                            )
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .track_scroll(&self.scroll_handle)
        .size_full()
    }

    fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        node: &SolutionTreeNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let project = self.project.clone();
        let node_clone = node.clone();
        let solution = self.state.solution.clone();
        let selected_nodes = self.state.selected_nodes.clone();
        let focus_handle = self.focus_handle.clone();
        let panel_entity = window.entity();

        let context_menu = ContextMenu::build(window, cx, move |menu, window, cx| {
            match &node_clone {
                SolutionTreeNode::Solution { path } => {
                    let solution_path = path.clone();
                    menu.context(focus_handle.clone())
                        .entry("Build Solution", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            window.dispatch_action(Spawn::ByName { task_name: "dotnet: build".to_string(), reveal_target: None }.boxed_clone(), cx);
                        }))
                        .entry("Rebuild Solution", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let task = SpawnInTerminal {
                                        command: Some("dotnet".to_string()),
                                        args: vec!["build".to_string(), "--no-incremental".to_string()],
                                        cwd: Some(root),
                                        ..Default::default()
                                    };
                                    workspace.spawn_in_terminal(task, window, cx).detach();
                                }
                            }).ok();
                        }))
                        .entry("Clean Solution", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            window.dispatch_action(Spawn::ByName { task_name: "dotnet: clean".to_string(), reveal_target: None }.boxed_clone(), cx);
                        }))
                        .separator()
                        .entry("Open Solution File", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path();
                                    let full_path = root.join(&solution_path);
                                    workspace.open_path(&full_path, OpenOptions::default(), cx);
                                }
                            }).ok();
                        }))
                }
                SolutionTreeNode::Project { name, path, guid } => {
                    let project_name = name.clone();
                    let project_path = path.clone();
                    let project_guid = guid.clone();
                    let is_startup = solution
                        .as_ref()
                        .and_then(|s| s.startup_project.as_ref())
                        .map(|sp| sp == guid)
                        .unwrap_or(false);

                    menu.context(focus_handle.clone())
                        .entry("Build", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let full_path = root.join(&project_path);
                                    let task = SpawnInTerminal {
                                        command: Some("dotnet".to_string()),
                                        args: vec!["build".to_string(), full_path.to_string_lossy().to_string()],
                                        cwd: Some(root),
                                        ..Default::default()
                                    };
                                    workspace.spawn_in_terminal(task, window, cx).detach();
                                }
                            }).ok();
                        }))
                        .entry("Rebuild", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let full_path = root.join(&project_path);
                                    let task = SpawnInTerminal {
                                        command: Some("dotnet".to_string()),
                                        args: vec!["build".to_string(), "--no-incremental".to_string(), full_path.to_string_lossy().to_string()],
                                        cwd: Some(root),
                                        ..Default::default()
                                    };
                                    workspace.spawn_in_terminal(task, window, cx).detach();
                                }
                            }).ok();
                        }))
                        .entry("Clean", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let full_path = root.join(&project_path);
                                    let task = SpawnInTerminal {
                                        command: Some("dotnet".to_string()),
                                        args: vec!["clean".to_string(), full_path.to_string_lossy().to_string()],
                                        cwd: Some(root),
                                        ..Default::default()
                                    };
                                    workspace.spawn_in_terminal(task, window, cx).detach();
                                }
                            }).ok();
                        }))
                        .separator()
                        .when(!is_startup, |menu| {
                            menu.entry("Set as Startup Project", None, window.handler_for(&panel_entity, move |this, window, cx| {
                                panel_entity.update(cx, |panel, cx| {
                                    if let Some(ref mut sol) = panel.state.solution {
                                        sol.startup_project = Some(project_guid.clone());
                                        cx.notify();
                                    }
                                });
                            }))
                        })
                        .when(is_startup, |menu| {
                            menu.entry("Unset Startup Project", None, window.handler_for(&panel_entity, move |this, window, cx| {
                                panel_entity.update(cx, |panel, cx| {
                                    if let Some(ref mut sol) = panel.state.solution {
                                        sol.startup_project = None;
                                        cx.notify();
                                    }
                                });
                            }))
                        })
                        .separator()
                        .entry("Open Project File", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path();
                                    let full_path = root.join(&project_path);
                                    workspace.open_path(&full_path, OpenOptions::default(), cx);
                                }
                            }).ok();
                        }))
                        .entry("Open Project Folder", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path();
                                    if let Some(parent) = project_path.parent() {
                                        let folder_path = root.join(parent);
                                        workspace.open_path(&folder_path, OpenOptions::default(), cx);
                                    }
                                }
                            }).ok();
                        }))
                        .separator()
                        .entry("Restore Packages", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let full_path = root.join(&project_path);
                                    let task = SpawnInTerminal {
                                        command: Some("dotnet".to_string()),
                                        args: vec!["restore".to_string(), full_path.to_string_lossy().to_string()],
                                        cwd: Some(root),
                                        ..Default::default()
                                    };
                                    workspace.spawn_in_terminal(task, window, cx).detach();
                                }
                            }).ok();
                        }))
                        .entry("Add Package...", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            // TODO: Show package search dialog
                            // For now, just show a placeholder message
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    let full_path = root.join(&project_path);
                                    // This would normally open a package search dialog
                                    // For now, we'll just show a message that this feature needs a dialog
                                    log::info!("Add Package dialog not yet implemented for project: {}", project_name);
                                }
                            }).ok();
                        }))
                        .separator()
                        .entry("Pack", None, window.handler_for(&panel_entity, move |this, window, cx| {
                            // Get selected projects or the clicked project
                            let projects_to_pack: Vec<_> = if let Some(ref sol) = this.state.solution {
                                let selected_projects: Vec<_> = selected_nodes
                                    .iter()
                                    .filter_map(|n| {
                                        if let SolutionTreeNode::Project { guid, .. } = n {
                                            sol.projects.iter().find(|p| p.guid == *guid)
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                
                                if !selected_projects.is_empty() {
                                    selected_projects
                                } else if let SolutionTreeNode::Project { guid, .. } = &node_clone {
                                    // Fall back to clicked project if no multi-selection
                                    sol.projects.iter().filter(|p| p.guid == *guid).collect()
                                } else {
                                    Vec::new()
                                }
                            } else {
                                Vec::new()
                            };
                            
                            if projects_to_pack.is_empty() {
                                return;
                            }
                            
                            let settings = SolutionExplorerSettings::get(None, cx);
                            let pack_output_dir = settings.pack_output_directory.clone();
                            
                            workspace.update(window, |workspace, cx| {
                                if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                    let root = worktree.read(cx).abs_path().to_path_buf();
                                    
                                    for project in projects_to_pack {
                                        let project_path = root.join(&project.path);
                                        let project_name = project.name.clone();
                                        
                                        // Build pack command arguments
                                        let mut pack_args = vec!["pack".to_string(), project_path.to_string_lossy().to_string()];
                                        
                                        // If output directory is configured, use --output flag
                                        // Otherwise, we'll pack to default location and copy afterwards
                                        let pack_output_dir_clone = if let Some(ref output_dir) = pack_output_dir {
                                            pack_args.push("--output".to_string());
                                            pack_args.push(output_dir.clone());
                                            None // Don't need to copy if using --output
                                        } else {
                                            pack_output_dir.clone() // Will copy after pack
                                        };
                                        
                                        // Run dotnet pack
                                        let task = SpawnInTerminal {
                                            command: Some("dotnet".to_string()),
                                            args: pack_args,
                                            cwd: Some(root.clone()),
                                            ..Default::default()
                                        };
                                        
                                        let task_result = workspace.spawn_in_terminal(task, window, cx);
                                        
                                        // If output directory was not used in --output, copy after pack completes
                                        if let Some(pack_output_dir) = pack_output_dir_clone {
                                            let project_path_clone = project_path.clone();
                                            let root_clone = root.clone();
                                            
                                            cx.spawn(async move |mut cx| {
                                                // Wait for pack to complete
                                                let exit_status = task_result.await.log_err().flatten();
                                                
                                                if exit_status.map(|s| s.success()).unwrap_or(false) {
                                                    // Find the generated .nupkg file
                                                    // Typically in bin/Debug or bin/Release/<project_name>.<version>.nupkg
                                                    let project_dir = project_path_clone.parent().unwrap_or(&root_clone);
                                                    
                                                    // Search for .nupkg files in bin directories
                                                    let mut found_nupkg = None;
                                                    
                                                    // Try common output locations
                                                    let bin_dir = project_dir.join("bin");
                                                    if bin_dir.exists() {
                                                        // Search in Debug and Release subdirectories
                                                        for config_dir in ["Debug", "Release"].iter() {
                                                            let config_path = bin_dir.join(config_dir);
                                                            if config_path.exists() {
                                                                if let Ok(entries) = std::fs::read_dir(&config_path) {
                                                                    for entry in entries.flatten() {
                                                                        let path = entry.path();
                                                                        if path.extension().and_then(|s| s.to_str()) == Some("nupkg") {
                                                                            found_nupkg = Some(path);
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            if found_nupkg.is_some() {
                                                                break;
                                                            }
                                                        }
                                                    }
                                    
                                                    if let Some(nupkg_path) = found_nupkg {
                                                        // Create output directory if it doesn't exist
                                                        let output_path = PathBuf::from(&pack_output_dir);
                                                        if let Err(e) = std::fs::create_dir_all(&output_path) {
                                                            log::error!("Failed to create pack output directory: {}", e);
                                                            return;
                                                        }
                                                        
                                                        // Copy the .nupkg file
                                                        let dest_path = output_path.join(nupkg_path.file_name().unwrap_or_default());
                                                        if let Err(e) = std::fs::copy(&nupkg_path, &dest_path) {
                                                            log::error!("Failed to copy .nupkg file: {}", e);
                                                        } else {
                                                            log::info!("Copied {} to {}", nupkg_path.display(), dest_path.display());
                                                        }
                                                    } else {
                                                        log::warn!("Could not find .nupkg file for project {}", project_name);
                                                    }
                                                }
                                            }).detach();
                                        }
                                    }
                                }
                            }).ok();
                        }))
                }
                SolutionTreeNode::Package { project_guid, package_id, version } => {
                    let package_id_clone = package_id.clone();
                    let project_guid_clone = project_guid.clone();
                    let project_path = solution
                        .as_ref()
                        .and_then(|s| s.projects.iter().find(|p| p.guid == *project_guid))
                        .map(|p| p.path.clone());
                    
                    if let Some(ref proj_path) = project_path {
                        let project_path_clone = proj_path.clone();
                        menu.context(focus_handle.clone())
                            .entry("Update Package", None, window.handler_for(&panel_entity, move |this, window, cx| {
                                workspace.update(window, |workspace, cx| {
                                    if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                        let root = worktree.read(cx).abs_path().to_path_buf();
                                        let full_path = root.join(&project_path_clone);
                                        let task = SpawnInTerminal {
                                            command: Some("dotnet".to_string()),
                                            args: vec!["add".to_string(), full_path.to_string_lossy().to_string(), "package".to_string(), package_id_clone.clone(), "--version".to_string(), "latest".to_string()],
                                            cwd: Some(root),
                                            ..Default::default()
                                        };
                                        workspace.spawn_in_terminal(task, window, cx).detach();
                                    }
                                }).ok();
                            }))
                            .entry("Remove Package", None, window.handler_for(&panel_entity, move |this, window, cx| {
                                workspace.update(window, |workspace, cx| {
                                    if let Some(worktree) = workspace.project().read(cx).worktrees().next() {
                                        let root = worktree.read(cx).abs_path().to_path_buf();
                                        let full_path = root.join(&project_path_clone);
                                        let task = SpawnInTerminal {
                                            command: Some("dotnet".to_string()),
                                            args: vec!["remove".to_string(), full_path.to_string_lossy().to_string(), "package".to_string(), package_id_clone.clone()],
                                            cwd: Some(root),
                                            ..Default::default()
                                        };
                                        workspace.spawn_in_terminal(task, window, cx).detach();
                                    }
                                }).ok();
                            }))
                    }
                }
            }
        });

        window.focus(&context_menu.focus_handle(cx));
        let subscription = cx.subscribe(&context_menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });

        self.context_menu = Some((context_menu, position, subscription));
        cx.notify();
    }
}

fn find_solution_file(root: &Path, fs: &dyn Fs) -> Option<PathBuf> {
    // Check current directory
    if let Ok(entries) = fs.read_dir(root) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".slnx") || name.ends_with(".sln") {
                        return Some(root.join(name));
                    }
                }
            }
        }
    }

    // Check parent directories (up to 3 levels)
    let mut current = root;
    for _ in 0..3 {
        if let Some(parent) = current.parent() {
            if let Ok(entries) = fs.read_dir(parent) {
                for entry in entries {
                    if let Ok(entry) = entry {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".slnx") || name.ends_with(".sln") {
                                return Some(parent.join(name));
                            }
                        }
                    }
                }
            }
            current = parent;
        } else {
            break;
        }
    }

    None
}

#[derive(Clone)]
struct TreeItem {
    node: SolutionTreeNode,
    label: String,
    icon: Option<IconName>,
    depth: usize,
    is_expanded: bool,
    has_children: bool,
}

impl Render for SolutionExplorerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_solution = self.state.solution.is_some();

        if has_solution {
            v_flex()
                .id("solution_explorer_panel")
                .size_full()
                .track_focus(&self.focus_handle)
                .child(
                    self.render_tree(cx)
                        .custom_scrollbars(
                            Scrollbars::default()
                                .tracked_scroll_handle(&self.scroll_handle)
                                .with_track_along(
                                    ScrollAxes::Horizontal,
                                    cx.theme().colors().panel_background,
                                )
                                .notify_content(),
                            window,
                            cx,
                        )
                        .size_full(),
                )
                .children(self.context_menu.as_ref().map(|(menu, position, _)| {
                    deferred(
                        anchored()
                            .position(*position)
                            .anchor(gpui::Corner::TopLeft)
                            .child(menu.clone()),
                    )
                    .with_priority(3)
                }))
        } else {
            v_flex()
                .id("empty-solution_explorer_panel")
                .p_4()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .track_focus(&self.focus_handle)
                .child(
                    Label::new("No .NET solution found")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        }
    }
}

impl EventEmitter<PanelEvent> for SolutionExplorerPanel {}

impl Panel for SolutionExplorerPanel {
    fn position(&self, _: &Window, cx: &App) -> DockPosition {
        match SolutionExplorerSettings::get_global(cx).dock {
            Some(DockSide::Left) => DockPosition::Left,
            Some(DockSide::Right) => DockPosition::Right,
            None => DockPosition::Left,
        }
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        settings::update_settings_file(self.fs.clone(), cx, move |settings, _| {
            let dock = match position {
                DockPosition::Left | DockPosition::Bottom => DockSide::Left,
                DockPosition::Right => DockSide::Right,
            };
            settings
                .solution_explorer
                .get_or_insert_default()
                .dock = Some(dock);
        });
    }

    fn size(&self, _: &Window, cx: &App) -> Pixels {
        self.width
            .unwrap_or_else(|| SolutionExplorerSettings::get_global(cx).default_width)
    }

    fn set_size(&mut self, size: Option<Pixels>, _: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::FileCode)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Solution Explorer")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn persistent_name() -> &'static str {
        "Solution Explorer"
    }

    fn panel_key() -> &'static str {
        SOLUTION_EXPLORER_PANEL_KEY
    }

    fn starts_open(&self, _: &Window, cx: &App) -> bool {
        SolutionExplorerSettings::get_global(cx).starts_open
    }

    fn activation_priority(&self) -> u32 {
        0
    }
}

impl Focusable for SolutionExplorerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SolutionExplorerPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<gpui::Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let panel = SolutionExplorerPanel::new(workspace.clone(), project, window, cx);
            Ok(panel)
        })
    }
}

