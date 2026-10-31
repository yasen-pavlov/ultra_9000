//! Frozen v2 IPC. Disk work runs on workers; GTK/AppKit/GL and workspace stay on main.
use crate::documents::{self, DocumentAsset, DocumentList, DocumentPreview, DocumentText};
use crate::{
    config::{SettingsState, WriteTarget},
    discovery::{self, Conversation, ConversationList},
    launch::{self, PathCheck},
    lifecycle,
    platform::{self, ModalGuard},
    runtime::{self, Request},
    workspace::{self, OpenableSheet, WorkspaceState},
};
use alacritty_embed::embed::ViewportRect;
use serde::Deserialize;
use std::{path::PathBuf, sync::mpsc};
use tauri::{AppHandle, Runtime};
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportRectDto {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    top_left_radius: f64,
    bottom_left_radius: f64,
}
fn on_main<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = tx.send(task());
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| "main-thread task dropped".into())
}
pub async fn on_main_deferred<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    platform::defer_on_main(Box::new(move || {
        let _ = tx.send(task());
    }));
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "main-thread task dropped".into())
}
async fn io<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| e.to_string())?
}
/// Execute the same command objects and preserve the same reply fields as the control link.
#[tauri::command]
pub async fn popover_command<R: Runtime>(
    app: AppHandle<R>,
    command: crate::control::Command,
) -> Result<crate::menubar::Reply, String> {
    let result = if matches!(command, crate::control::Command::RequestQuit {}) {
        Err(crate::control::Failure {
            error: "unsupported_command",
            message: "Quit is not a popover command.".into(),
        })
    } else {
        crate::control::execute(app, command).await
    };
    Ok(result.into())
}

#[tauri::command]
pub async fn popover_hide<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return on_main_deferred(move || crate::menubar::hide(&app)).await?;
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        Err("The menu-bar popover is available only on macOS.".into())
    }
}

#[tauri::command]
pub async fn popover_set_height<R: Runtime>(app: AppHandle<R>, height: f64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return on_main_deferred(move || crate::menubar::set_height(&app, height)).await?;
    #[cfg(target_os = "linux")]
    {
        let _ = (app, height);
        Err("The menu-bar popover is available only on macOS.".into())
    }
}

#[tauri::command]
pub async fn popover_toggle_main<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return on_main_deferred(move || lifecycle::toggle_main_inactive(&app)).await?;
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        Err("The menu-bar popover is available only on macOS.".into())
    }
}

#[tauri::command]
pub fn workspace_state<R: Runtime>(app: AppHandle<R>) -> Result<WorkspaceState, String> {
    on_main(&app, workspace::snapshot)?
}
pub async fn launch_item(
    tool: String,
    cwd: String,
    anchor: Option<String>,
    conversation: Option<Conversation>,
) -> Result<String, String> {
    let settings = on_main_deferred(workspace::settings).await??;
    if settings.tool(&tool).is_none() {
        return Err(format!("Unknown tool: {tool}"));
    }
    let (cwd, project) = io(move || {
        let cwd = launch::validate_directory(&cwd)?;
        let roots = discovery::normalize_roots(&settings.roots);
        let project = discovery::project_root(&cwd, &roots);
        if !roots.is_empty() && !roots.iter().any(|root| project.starts_with(root)) {
            return Err("Directory is outside projects.roots.".into());
        }
        Ok((
            cwd.to_string_lossy().into_owned(),
            project.to_string_lossy().into_owned(),
        ))
    })
    .await?;
    on_main_deferred(move || {
        workspace::begin_launch(tool, cwd, project, conversation, anchor, None, None)
    })
    .await?
}
#[tauri::command]
pub async fn item_launch(
    tool: String,
    cwd: String,
    anchor_item_id: Option<String>,
) -> Result<String, String> {
    launch_item(tool, cwd, anchor_item_id, None).await
}

#[tauri::command]
pub async fn open_document(path: String, select: bool, follow: bool) -> Result<String, String> {
    let settings = on_main_deferred(workspace::settings).await??;
    let (loaded, project) = io(move || {
        let (path, project) = documents::locate(std::path::Path::new(&path), &settings.roots)
            .map_err(|error| error.to_string())?;
        let loaded = documents::open(&path, &project, follow, workspace::now())?;
        Ok((loaded, project.to_string_lossy().into_owned()))
    })
    .await?;
    on_main_deferred(move || workspace::open_document(loaded, project, select)).await?
}

#[tauri::command]
pub async fn new_neovim_for_path(path: String) -> Result<String, String> {
    let settings = on_main_deferred(workspace::settings).await??;
    let (path, cwd, project) = io(move || {
        let (path, project) = documents::locate(std::path::Path::new(&path), &settings.roots)
            .map_err(|error| error.to_string())?;
        let directory = path.parent().ok_or("Document has no directory.")?;
        Ok((
            path.to_string_lossy().into_owned(),
            directory.to_string_lossy().into_owned(),
            project.to_string_lossy().into_owned(),
        ))
    })
    .await?;
    on_main_deferred(move || workspace::neovim_for_path(path, cwd, project)).await?
}
#[tauri::command]
pub async fn document_text(item_id: String) -> Result<DocumentText, String> {
    io(move || documents::text(item_id)).await
}
#[tauri::command]
pub async fn document_asset(item_id: String, rel_path: String) -> Result<DocumentAsset, String> {
    let path = on_main_deferred(move || workspace::document_path(&item_id)).await??;
    io(move || documents::asset(std::path::Path::new(&path), &rel_path)).await
}
#[tauri::command]
pub async fn document_reload(item_id: String) -> Result<(), String> {
    io(move || documents::reload(item_id)).await
}
#[tauri::command]
pub async fn project_documents(project_id: String) -> Result<DocumentList, String> {
    let (epoch, roots) = on_main_deferred(workspace::document_scan_context).await??;
    let mut list = io(move || {
        let directory = launch::validate_directory(&project_id)?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let roots = discovery::normalize_roots(&roots);
        if !roots.is_empty() && !roots.iter().any(|root| directory.starts_with(root)) {
            return Err("Directory is outside projects.roots.".into());
        }
        Ok(documents::project_documents(&directory, epoch))
    })
    .await?;
    on_main_deferred(move || {
        workspace::bind_document_entries(&mut list.entries)?;
        Ok::<_, String>(list)
    })
    .await?
}
#[tauri::command]
pub async fn document_preview(path: String) -> Result<DocumentPreview, String> {
    io(move || documents::preview(std::path::Path::new(&path))).await
}
#[tauri::command]
pub async fn new_neovim_for_file(item_id: String) -> Result<String, String> {
    on_main_deferred(move || workspace::new_neovim_for_file(&item_id)).await?
}
#[tauri::command]
pub async fn reveal_document(item_id: String) -> Result<(), String> {
    on_main_deferred(move || workspace::reveal_document(&item_id)).await?
}

#[tauri::command]
pub async fn document_open_link(item_id: String, url: String) -> Result<(), String> {
    let target = documents::external_link(&url)?;
    on_main_deferred(move || workspace::document_path(&item_id)).await??;
    io(move || runtime::open_file(std::path::Path::new(target.as_str()))).await
}
#[tauri::command]
pub async fn item_resume(
    tool: String,
    conversation_id: String,
    path: String,
) -> Result<String, String> {
    let t = tool.clone();
    let id = conversation_id.clone();
    let p = path.clone();
    if let Some(item) = on_main_deferred(move || workspace::select_bound(&t, &id, &p)).await?? {
        return Ok(item);
    }
    let c = resolve_resume(tool.clone(), conversation_id, path).await?;
    launch_item(tool, c.cwd.to_string_lossy().into(), None, Some(c)).await
}
/// Explicit references are re-read off-main, including hidden OMP stubs.
async fn resolve_resume(tool: String, id: String, path: String) -> Result<Conversation, String> {
    let settings = on_main_deferred(workspace::settings).await??;
    io(move || {
        let spec = settings
            .tool(&tool)
            .ok_or_else(|| format!("Unknown tool: {tool}"))?;
        discovery::read_bound(
            spec,
            &id,
            PathBuf::from(path).as_path(),
            &PathBuf::from(launch::default_directory()),
            &settings.roots,
        )
    })
    .await
}
#[tauri::command]
pub async fn item_restart(item_id: String) -> Result<String, String> {
    let id = item_id.clone();
    let (tool, cwd, project, conversation, args) =
        on_main_deferred(move || workspace::restart_target(&id)).await??;
    io({
        let cwd = cwd.clone();
        move || launch::validate_directory(&cwd).map(|_| ())
    })
    .await?;
    // Existing argv is retained for file-open Neovim; ordinary launches use current ToolSpec.
    on_main_deferred(move || {
        workspace::begin_launch(tool, cwd, project, conversation, None, Some(item_id), args)
    })
    .await?
}
#[tauri::command]
pub fn item_select<R: Runtime>(app: AppHandle<R>, item_id: String) -> Result<(), String> {
    on_main(&app, move || workspace::select(&item_id))?
}
#[tauri::command]
pub async fn item_close(item_id: String, confirmed: bool) -> Result<String, String> {
    on_main_deferred(move || workspace::close(&item_id, confirmed).map(str::to_owned)).await?
}
#[tauri::command]
pub fn notice_dismiss<R: Runtime>(app: AppHandle<R>, notice_id: String) -> Result<(), String> {
    on_main(&app, move || workspace::notice_dismiss(&notice_id))?
}
#[tauri::command]
pub fn sheet_open<R: Runtime>(app: AppHandle<R>, sheet: OpenableSheet) -> Result<(), String> {
    on_main(&app, move || workspace::sheet_open(sheet))?
}
#[tauri::command]
pub fn sheet_close<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    on_main(&app, workspace::sheet_close)?
}
#[tauri::command]
pub fn app_hide<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let handle = app.clone();
    on_main(&app, move || lifecycle::hide_window(&handle))?
}
#[tauri::command]
pub fn app_request_quit<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    lifecycle::request_quit(&app);
    Ok(())
}
#[tauri::command]
pub async fn app_quit<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    quit(move || app.exit(0)).await
}
/// Persist the last agent set, then stop every item and `exit`. A failed save cancels quitting.
async fn quit(exit: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let rx = on_main_deferred(|| workspace::quit_snapshot().and_then(runtime::save_ui)).await??;
    if let Err(e) = io(move || runtime::wait_reply(rx)).await {
        on_main_deferred(workspace::cancel_quitting).await?;
        return Err(e);
    }
    on_main_deferred(move || {
        workspace::shutdown_all();
        exit();
    })
    .await?;
    Ok(())
}
#[tauri::command]
pub async fn last_set_resume() -> Result<(), String> {
    let entries = on_main_deferred(workspace::last_set_entries).await??;
    for entry in entries {
        if entry.kind == crate::ui_state::LastSetKind::Document {
            if let Some(path) = entry.path {
                if let Err(error) = open_document(path, true, false).await {
                    on_main_deferred(move || workspace::action_error(error)).await?;
                }
            }
            continue;
        }
        let tool = entry.tool.clone();
        let id = entry.conversation_id.clone();
        let path = entry.conversation_path.clone();
        if on_main_deferred(move || workspace::select_bound(&tool, &id, &path))
            .await??
            .is_some()
        {
            continue;
        }
        let conversation = match resolve_resume(
            entry.tool.clone(),
            entry.conversation_id,
            entry.conversation_path,
        )
        .await
        {
            Ok(c) if !c.cwd_missing => c,
            Ok(_) => continue,
            Err(e) => {
                on_main_deferred(move || workspace::action_error(e)).await?;
                continue;
            }
        };
        let cwd = conversation.cwd.to_string_lossy().into_owned();
        if let Err(e) = launch_item(entry.tool, cwd, None, Some(conversation)).await {
            on_main_deferred(move || workspace::action_error(e)).await?;
        }
    }
    last_set_dismiss().await
}
#[tauri::command]
pub async fn last_set_dismiss() -> Result<(), String> {
    let rx =
        on_main_deferred(|| workspace::last_set_dismiss().and_then(runtime::save_ui)).await??;
    io(move || runtime::wait_reply(rx)).await
}
#[tauri::command]
pub async fn ui_set(
    sidebar_expanded: Option<bool>,
    folded_project_ids: Option<Vec<String>>,
) -> Result<(), String> {
    let rx = on_main_deferred(move || {
        workspace::ui_set(sidebar_expanded, folded_project_ids).and_then(runtime::save_ui)
    })
    .await??;
    io(move || runtime::wait_reply(rx)).await
}
#[tauri::command]
pub async fn path_check(path: String) -> Result<PathCheck, String> {
    io(move || Ok(launch::path_check(&path))).await
}
#[tauri::command]
pub async fn pick_directory(initial_directory: String) -> Result<Option<String>, String> {
    let initial = io(move || Ok(launch::validate_directory(&initial_directory).ok())).await?;
    on_main_deferred(move || {
        let guard = ModalGuard::acquire().ok_or("Another native dialog is open.")?;
        let chosen = platform::pick_directory(initial.as_deref());
        drop(guard);
        Ok(chosen)
    })
    .await?
}
#[tauri::command]
pub fn conversations_list<R: Runtime>(
    app: AppHandle<R>,
    project_id: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
) -> Result<ConversationList, String> {
    runtime::request_scan()?;
    on_main(&app, move || {
        workspace::conversations_list(project_id, query, limit)
    })?
}
#[tauri::command]
pub fn settings_state<R: Runtime>(app: AppHandle<R>) -> Result<SettingsState, String> {
    on_main(&app, workspace::settings_snapshot)?
}
#[tauri::command]
pub async fn settings_set(
    key: String,
    value: serde_json::Value,
    target: WriteTarget,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    runtime::send(Request::Set(key, Some(value), target, tx))?;
    io(move || runtime::wait_reply(rx)).await
}
#[tauri::command]
pub async fn settings_reset(key: String, target: WriteTarget) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    runtime::send(Request::Set(key, None, target, tx))?;
    io(move || runtime::wait_reply(rx)).await
}
#[tauri::command]
pub async fn settings_open_file(file: String, line: Option<u32>) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    runtime::send(Request::Starter(file, tx))?;
    let (cwd, file) = io(move || runtime::wait_reply(rx)).await?;
    let mut extra = Vec::new();
    if let Some(line) = line {
        extra.push(format!("+{}", line.max(1)));
    }
    extra.push(file);
    // Config files always belong to the config-dir project, even if it lives inside a git repo.
    on_main_deferred(move || {
        workspace::begin_launch(
            "neovim".into(),
            cwd.clone(),
            cwd,
            None,
            None,
            None,
            Some(extra),
        )
    })
    .await??;
    Ok(())
}
#[tauri::command]
pub async fn settings_reveal() -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    runtime::send(Request::Reveal(tx))?;
    io(move || runtime::wait_reply(rx)).await
}
#[tauri::command]
pub fn terminal_set_viewport<R: Runtime>(
    app: AppHandle<R>,
    item_id: String,
    presentation_epoch: u64,
    rect: ViewportRectDto,
    visible: bool,
    sequence: u64,
) -> Result<bool, String> {
    on_main(&app, move || {
        workspace::set_viewport(
            &item_id,
            presentation_epoch,
            ViewportRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                top_left_radius: rect.top_left_radius,
                bottom_left_radius: rect.bottom_left_radius,
            },
            visible,
            sequence,
        )
    })?
}
#[tauri::command]
pub fn terminal_focus<R: Runtime>(
    app: AppHandle<R>,
    item_id: String,
    presentation_epoch: u64,
    sequence: u64,
) -> Result<bool, String> {
    on_main(&app, move || {
        workspace::focus(&item_id, presentation_epoch, sequence)
    })?
}
/// Headless stand-in for the app's main thread (Linux): glib's default main context, which
/// `platform::defer_on_main` targets, is iterated by the one test thread holding [`Main`].
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_support;

#[cfg(all(test, target_os = "linux"))]
mod tests;
