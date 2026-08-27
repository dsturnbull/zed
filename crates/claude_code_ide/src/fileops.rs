//! File-operation MCP tools: openFile, saveDocument, checkDocumentDirty, close_tab.
//! These drive the workspace UI, so they run through the workspace's window handle
//! rather than a bare `App`.
//!
//! `openDiff` is deliberately absent: an interactive keep/reject surface is separate
//! work, and `server.rs` answers it with a non-hanging acknowledgement meanwhile.

use gpui::{AppContext as _, AsyncApp, Entity};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::{Path, PathBuf};
use workspace::Workspace;

use crate::protocol::{FilePathArg, OpenFileArgs, tool_error, tool_ok};
use crate::selection::SharedSelection;

/// Deserialise a tool's arguments, or hand back the error to return.
fn args_of<T: DeserializeOwned>(tool: &str, arguments: &Value) -> Result<T, Value> {
    serde_json::from_value(arguments.clone())
        .map_err(|err| tool_error(format!("bad {tool} args: {err}")))
}

/// The bound workspace, or the error to return.
fn workspace_of(selection: &SharedSelection) -> Result<gpui::WeakEntity<Workspace>, Value> {
    selection
        .workspace()
        .ok_or_else(|| tool_error("no workspace bound"))
}

/// A tool's target path plus the project to resolve it in. The shared preamble of
/// every path-taking tool here.
fn path_and_project(
    tool: &str,
    selection: &SharedSelection,
    arguments: &Value,
    cx: &mut AsyncApp,
) -> Result<(PathBuf, Entity<project::Project>), Value> {
    let args: FilePathArg = args_of(tool, arguments)?;
    let workspace = workspace_of(selection)?;
    let project = workspace
        .read_with(cx, |workspace, _| workspace.project().clone())
        .map_err(|err| tool_error(format!("workspace gone: {err}")))?;
    Ok((uri_to_path(&args.file_path), project))
}

/// `openFile`: open a path in the workspace, optionally selecting a line range.
pub async fn open_file(
    selection: &SharedSelection,
    arguments: &Value,
    cx: &mut AsyncApp,
) -> Value {
    let args: OpenFileArgs = match args_of("openFile", arguments) {
        Ok(args) => args,
        Err(err) => return err,
    };
    let (Some(workspace), Some(window)) = (selection.workspace(), selection.window()) else {
        return tool_error("no workspace bound");
    };
    let abs_path = uri_to_path(&args.file_path);

    // open_abs_path needs a Window + Context<Workspace>, so drive it on the
    // workspace's window.
    let open_task = cx.update_window(window, |_root, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_abs_path(
                abs_path.clone(),
                workspace::OpenOptions::default(),
                window,
                cx,
            )
        })
    });
    let open_task = match open_task {
        Ok(Ok(task)) => task,
        Ok(Err(err)) => return tool_error(format!("workspace gone: {err}")),
        Err(err) => return tool_error(format!("window gone: {err}")),
    };
    let item = match open_task.await {
        Ok(item) => item,
        Err(err) => return tool_error(format!("open failed: {err}")),
    };

    if let Some(start_line) = args.start_line {
        let end_line = args.end_line.unwrap_or(start_line);
        let _ = cx.update_window(window, |_root, window, cx| {
            if let Some(editor) = item.downcast::<editor::Editor>() {
                editor.update(cx, |editor, cx| {
                    select_lines(editor, start_line, end_line, window, cx);
                });
            }
        });
    }

    tool_ok(format!("opened {}", args.file_path))
}

fn select_lines(
    editor: &mut editor::Editor,
    start_line: u32,
    end_line: u32,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<editor::Editor>,
) {
    let snapshot = editor.snapshot(window, cx);
    let start = language::Point::new(start_line, 0);
    let end_col = snapshot
        .buffer_snapshot()
        .line_len(multi_buffer::MultiBufferRow(end_line));
    let end = language::Point::new(end_line, end_col);
    editor.change_selections(editor::SelectionEffects::default(), window, cx, |selections| {
        selections.select_ranges([start..end]);
    });
}

/// `saveDocument`: persist the buffer for a path if it is open.
pub async fn save_document(
    selection: &SharedSelection,
    arguments: &Value,
    cx: &mut AsyncApp,
) -> Value {
    let (abs_path, project) = match path_and_project("saveDocument", selection, arguments, cx) {
        Ok(pair) => pair,
        Err(err) => return err,
    };
    let open_task = project.update(cx, |project, cx| {
        project
            .find_project_path(&abs_path, cx)
            .map(|project_path| project.open_buffer(project_path, cx))
    });
    let Some(open_task) = open_task else {
        return tool_error(format!("path not found in project: {}", abs_path.display()));
    };
    let buffer = match open_task.await {
        Ok(buffer) => buffer,
        Err(err) => return tool_error(format!("open buffer failed: {err}")),
    };
    let save_task = project.update(cx, |project, cx| project.save_buffer(buffer, cx));
    match save_task.await {
        Ok(()) => tool_ok("saved"),
        Err(err) => tool_error(format!("save failed: {err}")),
    }
}

/// `checkDocumentDirty`: report whether a path's buffer has unsaved changes.
pub async fn check_document_dirty(
    selection: &SharedSelection,
    arguments: &Value,
    cx: &mut AsyncApp,
) -> Value {
    let (abs_path, project) = match path_and_project("checkDocumentDirty", selection, arguments, cx)
    {
        Ok(pair) => pair,
        Err(err) => return err,
    };
    // Only report on an already-open buffer: opening one to check would always
    // report clean.
    let existing = project.update(cx, |project, cx| {
        project
            .find_project_path(&abs_path, cx)
            .and_then(|project_path| project.get_open_buffer(&project_path, cx))
    });
    let Some(buffer) = existing else {
        return tool_ok(r#"{"isDirty":false,"isOpen":false}"#);
    };
    let is_dirty = buffer.read_with(cx, |buffer, _| buffer.is_dirty());
    tool_ok(format!(r#"{{"isDirty":{is_dirty},"isOpen":true}}"#))
}

/// `close_tab`: close the tab whose item resolves to the named path.
pub async fn close_tab(
    selection: &SharedSelection,
    arguments: &Value,
    cx: &mut AsyncApp,
) -> Value {
    let args: FilePathArg = match args_of("close_tab", arguments) {
        Ok(args) => args,
        Err(err) => return err,
    };
    let (Some(workspace), Some(window)) = (selection.workspace(), selection.window()) else {
        return tool_error("no workspace bound");
    };
    let target = uri_to_path(&args.file_path);

    let result = cx.update_window(window, |_root, window, cx| {
        workspace.update(cx, |workspace, cx| {
            close_matching_tab(workspace, &target, window, cx)
        })
    });
    match result {
        Ok(Ok(true)) => tool_ok("closed"),
        Ok(Ok(false)) => tool_ok("no matching tab"),
        Ok(Err(err)) => tool_error(format!("workspace gone: {err}")),
        Err(err) => tool_error(format!("window gone: {err}")),
    }
}

fn close_matching_tab(
    workspace: &mut Workspace,
    target: &Path,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Workspace>,
) -> bool {
    let matching = workspace.items(cx).find(|item| {
        item.project_path(cx)
            .and_then(|project_path| {
                workspace
                    .project()
                    .read(cx)
                    .worktree_for_id(project_path.worktree_id, cx)
                    .map(|worktree| worktree.read(cx).absolutize(&project_path.path))
            })
            .is_some_and(|path| path == target)
    });
    let Some(item_id) = matching.map(|item| item.item_id()) else {
        return false;
    };
    let pane = workspace.active_pane().clone();
    pane.update(cx, |pane, cx| {
        pane.close_item_by_id(item_id, workspace::SaveIntent::Close, window, cx)
            .detach();
    });
    true
}

/// Turn a `file://` URI (or a bare path) into a filesystem path.
fn uri_to_path(uri: &str) -> PathBuf {
    let trimmed = uri.strip_prefix("file://").unwrap_or(uri);
    Path::new(trimmed).to_path_buf()
}
