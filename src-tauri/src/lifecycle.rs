//! Hide retains every item; explicit quit persists the last agent set, then bounded shutdown.
#[cfg(target_os = "macos")]
use crate::shortcuts::Action;
use crate::{
    platform,
    tray_model::TrayModel,
    workspace::{self, WorkspaceState},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "macos")]
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    Wry,
};
use tauri::{App, AppHandle, Manager, Runtime};
pub const MAIN_WINDOW: &str = "main";
#[cfg(target_os = "macos")]
const TRAY_SHOW: &str = "tray-show";
#[cfg(target_os = "macos")]
const TRAY_HIDE: &str = "tray-hide";
#[cfg(target_os = "macos")]
const TRAY_QUIT: &str = "tray-quit";
#[cfg(target_os = "macos")]
const TRAY_ITEM: &str = "tray-item-";
#[cfg(target_os = "macos")]
const TRAY_MORE: &str = "tray-more";
#[cfg(target_os = "macos")]
const TRAY_NOTIFY: &str = "tray-notify";
thread_local! {
    static TRAY_MODEL: RefCell<Option<TrayModel>> = const { RefCell::new(None) };
}
static QUIT_PENDING: AtomicBool = AtomicBool::new(false);
/// The window's shown/hidden state as last requested by the app. `Window::hide`/`show` are
/// applied asynchronously, so `is_visible()` read right after them is stale; only the app
/// hides its own window (compositor close requests route through `hide_window`).
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(true);
#[cfg(target_os = "macos")]
pub struct MenuBarIcon<R: Runtime>(pub TrayIcon<R>);
#[cfg(target_os = "macos")]
pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    use tauri::menu::Submenu;
    let keys = crate::shortcuts::Keymap::defaults(crate::runtime::PLATFORM);
    let app_menu = Submenu::with_items(
        app,
        "Ultra 9000",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Ultra 9000"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "hide", "Hide Ultra 9000", true, None::<&str>)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Ultra 9000", true, None::<&str>)?,
        ],
    )?;
    let items = Submenu::new(app, "Items", true)?;
    for action in Action::all()
        .into_iter()
        .filter(|a| !matches!(a, Action::Hide | Action::Quit))
    {
        let label = keys
            .label(action)
            .map(|key| format!("{}    {key}", action.name()))
            .unwrap_or_else(|| action.name());
        items.append(&MenuItem::with_id(
            app,
            action.name(),
            label,
            true,
            None::<&str>,
        )?)?;
    }
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &items, &edit])
}
fn key_router() -> Rc<dyn Fn(&alacritty_embed::embed::AppKey) -> bool> {
    Rc::new(move |key| {
        let Some(action) = workspace::key_action(key) else {
            return false;
        };
        platform::defer_on_main(Box::new(move || {
            if let Err(e) = workspace::dispatch(action) {
                workspace::action_error(e);
            }
        }));
        true
    })
}
#[cfg(target_os = "linux")]
pub fn install_shortcuts(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use gtk::prelude::Cast;
    let window = window.gtk_window()?;
    platform::install_key_router(window.upcast_ref(), key_router());
    Ok(())
}
#[cfg(target_os = "macos")]
pub fn install_macos_key_router(window: platform::MainWindow) {
    platform::install_key_router(window, key_router());
}
#[cfg(target_os = "macos")]
fn tray_menu<R: Runtime>(app: &AppHandle<R>, model: &TrayModel) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "tray-header",
        &model.header,
        false,
        None::<&str>,
    )?)?;
    for entry in &model.entries {
        menu.append(&MenuItem::with_id(
            app,
            format!("{TRAY_ITEM}{}", entry.id),
            format!("{}    {}", entry.label, entry.detail),
            true,
            None::<&str>,
        )?)?;
    }
    if model.more_running > 0 {
        menu.append(&MenuItem::with_id(
            app,
            TRAY_MORE,
            format!("+ {} more running", model.more_running),
            true,
            None::<&str>,
        )?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let label = menu_label(
        model.visibility_label(),
        if model.visible {
            model.hide_shortcut.as_deref()
        } else {
            None
        },
    );
    menu.append(&MenuItem::with_id(
        app,
        if model.visible { TRAY_HIDE } else { TRAY_SHOW },
        label,
        true,
        None::<&str>,
    )?)?;
    menu.append(&CheckMenuItem::with_id(
        app,
        TRAY_NOTIFY,
        crate::tray_model::NOTIFY_LABEL,
        true,
        model.notify_checked,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        TRAY_QUIT,
        menu_label(
            crate::tray_model::QUIT_LABEL,
            model.quit_shortcut.as_deref(),
        ),
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}
#[cfg(target_os = "macos")]
fn menu_label(label: &str, shortcut: Option<&str>) -> String {
    shortcut.map_or_else(|| label.to_owned(), |key| format!("{label}    {key}"))
}
fn model(state: &WorkspaceState) -> TrayModel {
    let visible = WINDOW_SHOWN.load(Ordering::Acquire);
    #[cfg(target_os = "macos")]
    let visible = visible && !platform::application_hidden();
    let mut model = TrayModel::build(
        state,
        visible,
        workspace::notify_needs_you().unwrap_or(false),
    );
    model.dark_icon = workspace::tray_dark_icon().unwrap_or(false);
    #[cfg(target_os = "macos")]
    if let Ok(app) = workspace::app_handle() {
        // Coloured attention images must follow the menu bar's actual appearance.
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            if let Ok(theme) = window.theme() {
                model.dark_icon = theme == tauri::Theme::Light;
            }
        }
    }
    model
}
#[cfg(target_os = "macos")]
fn macos_tray_image(model: &TrayModel) -> tauri::Result<Image<'static>> {
    let bytes: &[u8] = if !model.attention() {
        include_bytes!("../icons/tray/macos-template-36.png")
    } else if model.dark_icon {
        include_bytes!("../icons/tray/macos-dark-attention-36.png")
    } else {
        include_bytes!("../icons/tray/macos-light-attention-36.png")
    };
    Image::from_bytes(bytes)
}
pub fn install_menu_bar_icon(app: &App) -> tauri::Result<()> {
    let state = workspace::snapshot().map_err(|e| tauri::Error::Io(std::io::Error::other(e)))?;
    let model = model(&state);
    #[cfg(target_os = "linux")]
    crate::tray_linux::install(app.handle(), model.clone())?;
    #[cfg(target_os = "macos")]
    {
        let menu = tray_menu(app.handle(), &model)?;
        let tray = TrayIconBuilder::with_id("ultra9000")
            .icon(macos_tray_image(&model)?)
            .icon_as_template(!model.attention())
            .tooltip(&model.tooltip)
            .menu(&menu)
            .show_menu_on_left_click(true)
            .build(app)?;
        app.manage(MenuBarIcon(tray));
    }
    TRAY_MODEL.with(|cache| *cache.borrow_mut() = Some(model));
    Ok(())
}
pub fn update_tray<R: Runtime>(app: &AppHandle<R>, state: &WorkspaceState) {
    let model = model(state);
    if TRAY_MODEL.with(|cache| cache.borrow().as_ref() == Some(&model)) {
        return;
    }
    #[cfg(target_os = "linux")]
    crate::tray_linux::update(app, model.clone());
    #[cfg(target_os = "macos")]
    if let Some(tray) = app.try_state::<MenuBarIcon<R>>() {
        let result = (|| {
            tray.0.set_menu(Some(tray_menu(app, &model)?))?;
            tray.0.set_tooltip(Some(&model.tooltip))?;
            tray.0.set_icon(Some(macos_tray_image(&model)?))?;
            tray.0.set_icon_as_template(!model.attention())?;
            Ok::<(), tauri::Error>(())
        })();
        if let Err(error) = result {
            log::warn!("tray menu: {error}");
            return;
        }
    }
    TRAY_MODEL.with(|cache| *cache.borrow_mut() = Some(model));
}
pub fn refresh_tray<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(state) = workspace::snapshot() {
        update_tray(app, &state);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Hide,
    Select(String),
    More,
    ToggleNotify,
    Quit,
}
pub fn handle_tray_action<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    let app = app.clone();
    platform::defer_on_main(Box::new(move || {
        let result = match action {
            TrayAction::Show => show_window(&app),
            TrayAction::Hide => hide_window(&app),
            TrayAction::Select(id) => show_window(&app).and_then(|()| workspace::select(&id)),
            TrayAction::More => show_window(&app).and_then(|()| {
                workspace::sheet_open(workspace::OpenableSheet::Palette {
                    project_id: None,
                    mode: workspace::PaletteMode::Open,
                })
            }),
            TrayAction::ToggleNotify => {
                let value = workspace::notify_needs_you().map(|enabled| !enabled);
                match value {
                    Ok(value) => {
                        tauri::async_runtime::spawn(async move {
                            if let Err(error) = crate::commands::settings_set(
                                "notify.needs_you".into(),
                                serde_json::json!(value),
                                crate::config::WriteTarget::Auto,
                            )
                            .await
                            {
                                platform::defer_on_main(Box::new(move || {
                                    workspace::action_error(error)
                                }));
                            }
                        });
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            TrayAction::Quit => {
                request_quit(&app);
                Ok(())
            }
        };
        if let Err(error) = result {
            workspace::action_error(error);
        }
    }));
}
#[cfg(target_os = "macos")]
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    let action = match id {
        TRAY_SHOW => Some(TrayAction::Show),
        TRAY_HIDE => Some(TrayAction::Hide),
        TRAY_QUIT => Some(TrayAction::Quit),
        TRAY_MORE => Some(TrayAction::More),
        TRAY_NOTIFY => Some(TrayAction::ToggleNotify),
        id if id.starts_with(TRAY_ITEM) => Some(TrayAction::Select(id[TRAY_ITEM.len()..].into())),
        _ => None,
    };
    if let Some(action) = action {
        handle_tray_action(app, action);
        return;
    }
    let id = id.to_owned();
    platform::defer_on_main(Box::new(move || {
        let result = Action::parse(&id).map_or(Ok(()), workspace::dispatch);
        if let Err(e) = result {
            workspace::action_error(e);
        }
    }));
}
pub fn hide_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    workspace::invalidate_focus();
    app.get_webview_window(MAIN_WINDOW)
        .ok_or("main window not found")?
        .hide()
        .map_err(|e| e.to_string())?;
    WINDOW_SHOWN.store(false, Ordering::Release);
    refresh_tray(app);
    Ok(())
}
pub fn hide_app<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        workspace::invalidate_focus();
        platform::hide_application();
        refresh_tray(app);
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        hide_window(app)
    }
}
pub fn show_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("main window not found")?;
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(|e| e.to_string())?;
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    WINDOW_SHOWN.store(true, Ordering::Release);
    refresh_tray(app);
    Ok(())
}
pub fn request_quit<R: Runtime>(app: &AppHandle<R>) {
    if QUIT_PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    platform::defer_on_main(Box::new(move || {
        let result = workspace::live_items().and_then(|live| {
            if live.is_empty() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::app_quit(app).await {
                        platform::defer_on_main(Box::new(move || workspace::action_error(e)));
                    }
                });
                Ok(())
            } else {
                show_window(&app)?;
                workspace::open_quit()
            }
        });
        QUIT_PENDING.store(false, Ordering::Release);
        if let Err(e) = result {
            workspace::action_error(e);
        }
    }));
}
#[cfg(target_os = "macos")]
pub fn confirm_and_stop_sessions() -> bool {
    // AppKit must cancel its synchronous termination; the async confirmed path owns exit.
    if let Ok(app) = workspace::app_handle() {
        request_quit(&app);
    }
    false
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
