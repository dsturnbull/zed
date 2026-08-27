//! Read-only workspace queries backing the `getDiagnostics` and
//! `getOpenEditors` MCP tools. These read project/editor state through the
//! workspace handle the bridge stashed in [`SharedSelection`].

use gpui::{App, AsyncApp};
use language::{DiagnosticSeverity, OffsetRangeExt as _};
use std::path::Path;
use workspace::Workspace;

use crate::protocol::{tool_error, tool_ok};
use crate::selection::SharedSelection;
use serde_json::{Value, json};

/// Build the `getDiagnostics` result. With a `file://` URI (or plain path) in
/// `arguments.uri`, scope to that file; otherwise summarise the whole project.
pub async fn diagnostics_result(
    selection: &SharedSelection,
    arguments: &serde_json::Value,
    cx: &mut AsyncApp,
) -> serde_json::Value {
    let Some(workspace) = selection.workspace() else {
        return tool_error("no workspace bound");
    };

    // The protocol passes the target as a file URI under `uri`; tolerate a bare
    // path too. Absent means project-wide.
    let path = arguments
        .get("uri")
        .and_then(|v| v.as_str())
        .map(uri_to_path);

    let project = match workspace.read_with(cx, |workspace, _| workspace.project().clone()) {
        Ok(project) => project,
        Err(err) => return tool_error(format!("workspace gone: {err}")),
    };

    if let Some(path) = path {
        // Per-file: resolve the path in the project, open the buffer, read its
        // diagnostic groups. `Entity::update`/`read_with` on AsyncApp return the
        // closure value directly (the handle keeps the app alive), unlike the
        // fallible WeakEntity variants.
        let open_task = project.update(cx, |project, cx| {
            project
                .find_project_path(&path, cx)
                .map(|project_path| project.open_buffer(project_path, cx))
        });
        let Some(open_task) = open_task else {
            return tool_error(format!(
                "path not found in project: {}",
                path.display()
            ));
        };
        let buffer = match open_task.await {
            Ok(buffer) => buffer,
            Err(err) => return tool_error(format!("open buffer failed: {err}")),
        };

        let path_str = path.to_string_lossy().to_string();
        let diagnostics = buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            snapshot
                .diagnostic_groups(None)
                .into_iter()
                .map(|(_, group)| {
                    let entry = &group.entries[group.primary_ix];
                    let range = entry.range.to_point(&snapshot);
                    json!({
                        "filePath": path_str,
                        "severity": severity_str(entry.diagnostic.severity),
                        "line": range.start.row,
                        "character": range.start.column,
                        "message": entry.diagnostic.message,
                        "source": entry.diagnostic.source,
                    })
                })
                .collect::<Vec<_>>()
        });
        encode(&json!({ "diagnostics": diagnostics }))
    } else {
        // Project-wide, one entry per file with counts folded into the message, so
        // this never has to open a buffer.
        let diagnostics = cx.update(|cx| project_wide(&project, cx));
        encode(&json!({ "diagnostics": diagnostics }))
    }
}

fn project_wide(project: &gpui::Entity<project::Project>, cx: &App) -> Vec<Value> {
    let project = project.read(cx);
    let mut items = Vec::new();
    for (project_path, _, summary) in project.diagnostic_summaries(true, cx) {
        if summary.error_count == 0 && summary.warning_count == 0 {
            continue;
        }
        let Some(worktree) = project.worktree_for_id(project_path.worktree_id, cx) else {
            continue;
        };
        let abs = worktree.read(cx).absolutize(&project_path.path);
        items.push(json!({
            "filePath": abs.to_string_lossy(),
            "severity": if summary.error_count > 0 { "error" } else { "warning" },
            "line": 0,
            "character": 0,
            "message": format!(
                "{} error(s), {} warning(s)",
                summary.error_count, summary.warning_count
            ),
        }));
    }
    items
}

/// Build the `getOpenEditors` result: every open editor tab across all panes.
pub fn open_editors_result(selection: &SharedSelection, cx: &mut AsyncApp) -> serde_json::Value {
    let Some(workspace) = selection.workspace() else {
        return tool_error("no workspace bound");
    };
    let result = cx.update(|cx| {
        workspace
            .read_with(cx, |workspace, cx| collect_open_editors(workspace, cx))
            .ok()
    });
    match result {
        Some(open_editors) => encode(&json!({ "openEditors": open_editors })),
        None => tool_error("workspace gone"),
    }
}

fn collect_open_editors(workspace: &Workspace, cx: &App) -> Vec<Value> {
    let active_id = workspace.active_item(cx).map(|item| item.item_id());
    let mut editors = Vec::new();
    for item in workspace.items(cx) {
        let Some(path) = item
            .project_path(cx)
            .and_then(|project_path| abs_path_for(workspace, &project_path, cx))
        else {
            continue;
        };
        let file_path = path.to_string_lossy().to_string();
        editors.push(json!({
            "filePath": file_path,
            "fileUrl": format!("file://{file_path}"),
            "isActive": Some(item.item_id()) == active_id,
            "isDirty": item.is_dirty(cx),
        }));
    }
    editors
}

/// Absolutise a project-relative path via its worktree.
fn abs_path_for(
    workspace: &Workspace,
    project_path: &project::ProjectPath,
    cx: &App,
) -> Option<std::path::PathBuf> {
    let worktree = workspace
        .project()
        .read(cx)
        .worktree_for_id(project_path.worktree_id, cx)?;
    Some(worktree.read(cx).absolutize(&project_path.path))
}

fn severity_str(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::ERROR => "error",
        DiagnosticSeverity::WARNING => "warning",
        DiagnosticSeverity::INFORMATION => "information",
        DiagnosticSeverity::HINT => "hint",
        _ => "info",
    }
}

/// Turn a `file://` URI (or a bare path) into a filesystem path.
fn uri_to_path(uri: &str) -> std::path::PathBuf {
    let trimmed = uri.strip_prefix("file://").unwrap_or(uri);
    Path::new(trimmed).to_path_buf()
}

fn encode<T: serde::Serialize>(value: &T) -> serde_json::Value {
    match serde_json::to_string(value) {
        Ok(text) => tool_ok(text),
        Err(err) => tool_error(format!("failed to encode: {err}")),
    }
}
