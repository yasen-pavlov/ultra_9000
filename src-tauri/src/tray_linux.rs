//! Pure Rust StatusNotifierItem/dbusmenu; no GTK AppIndicator dependency.
use crate::{
    lifecycle::{self, TrayAction},
    tray_model::{TrayModel, NOTIFY_LABEL, QUIT_LABEL},
};
use ksni::{
    menu::{CheckmarkItem, StandardItem},
    Icon, MenuItem, Status, ToolTip, TrayMethods,
};
use tauri::{image::Image, AppHandle, Manager, Runtime};
use tokio::sync::watch;

struct LinuxTray {
    /// Dispatches a chosen menu/tray action (production: `lifecycle::handle_tray_action`).
    on_action: Box<dyn Fn(TrayAction) + Send>,
    model: TrayModel,
    light: IconSet,
    dark: IconSet,
}

struct IconSet {
    normal: Vec<Icon>,
    attention: Vec<Icon>,
}

impl LinuxTray {
    fn icons(&self) -> &IconSet {
        if self.model.dark_icon {
            &self.dark
        } else {
            &self.light
        }
    }

    fn action(&self, action: TrayAction) {
        (self.on_action)(action);
    }

    fn item(label: &str, action: TrayAction, shortcut: Option<&str>) -> MenuItem<Self> {
        StandardItem {
            // dbusmenu treats underscores as mnemonic markers; app titles/glyphs are literal.
            label: label.replace('_', "__"),
            shortcut: shortcut_property(shortcut),
            activate: Box::new(move |tray: &mut Self| tray.action(action.clone())),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for LinuxTray {
    const MENU_ON_ACTIVATE: bool = false;
    fn id(&self) -> String {
        "ultra9000".into()
    }
    fn title(&self) -> String {
        "Ultra 9000".into()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.action(TrayAction::Show);
    }
    fn status(&self) -> Status {
        if self.model.attention() {
            Status::NeedsAttention
        } else {
            Status::Active
        }
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        if self.model.attention() {
            self.icons().attention.clone()
        } else {
            self.icons().normal.clone()
        }
    }
    fn attention_icon_pixmap(&self) -> Vec<Icon> {
        self.icons().attention.clone()
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.model.tooltip.clone(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut menu = Vec::with_capacity(11);
        menu.push(
            StandardItem {
                label: self.model.header.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );
        for entry in &self.model.entries {
            menu.push(Self::item(
                &entry.label,
                TrayAction::Select(entry.id.clone()),
                None,
            ));
        }
        if self.model.more_running > 0 {
            menu.push(Self::item(
                &format!("+ {} more running", self.model.more_running),
                TrayAction::More,
                None,
            ));
        }
        menu.push(MenuItem::Separator);
        menu.push(Self::item(
            self.model.visibility_label(),
            if self.model.visible {
                TrayAction::Hide
            } else {
                TrayAction::Show
            },
            if self.model.visible {
                self.model.hide_shortcut.as_deref()
            } else {
                None
            },
        ));
        menu.push(
            CheckmarkItem {
                label: NOTIFY_LABEL.into(),
                checked: self.model.notify_checked,
                activate: Box::new(|tray: &mut Self| tray.action(TrayAction::ToggleNotify)),
                ..Default::default()
            }
            .into(),
        );
        menu.push(MenuItem::Separator);
        menu.push(Self::item(
            QUIT_LABEL,
            TrayAction::Quit,
            self.model.quit_shortcut.as_deref(),
        ));
        menu
    }
}

fn shortcut_property(label: Option<&str>) -> Vec<Vec<String>> {
    label
        .map(|label| {
            vec![label
                .split('+')
                .map(|part| {
                    match part {
                        "Ctrl" => "Control",
                        "Cmd" => "Super",
                        other => other,
                    }
                    .to_owned()
                })
                .collect()]
        })
        .unwrap_or_default()
}

fn icon(bytes: &[u8]) -> tauri::Result<Icon> {
    let image = Image::from_bytes(bytes)?;
    let mut data = image.rgba().to_vec();
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width: image.width() as i32,
        height: image.height() as i32,
        data,
    })
}

struct LinuxTrayHandle(watch::Sender<TrayModel>);

/// Light and dark tray icon sets, each in every bundled size.
fn icon_sets() -> tauri::Result<(IconSet, IconSet)> {
    macro_rules! icons {
        ($variant:literal, $suffix:literal) => {
            [
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-16.png")).as_slice(),
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-20.png")).as_slice(),
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-22.png")).as_slice(),
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-24.png")).as_slice(),
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-32.png")).as_slice(),
                include_bytes!(concat!("../icons/tray/", $variant, $suffix, "-40.png")).as_slice(),
            ]
            .into_iter()
            .map(icon)
            .collect::<tauri::Result<Vec<_>>>()?
        };
    }
    let light = IconSet {
        normal: icons!("light", ""),
        attention: icons!("light", "-attention"),
    };
    let dark = IconSet {
        normal: icons!("dark", ""),
        attention: icons!("dark", "-attention"),
    };
    Ok((light, dark))
}

pub fn install(app: &AppHandle, model: TrayModel) -> tauri::Result<()> {
    let (light, dark) = icon_sets()?;
    let mut rx = register(app, model.clone());
    let app = app.clone();
    let tray = LinuxTray {
        on_action: Box::new(move |action| lifecycle::handle_tray_action(&app, action)),
        model,
        light,
        dark,
    };
    tauri::async_runtime::spawn(async move {
        // Stay registered even if the desktop's watcher starts after the application.
        let handle = match tray.assume_sni_available(true).spawn().await {
            Ok(handle) => handle,
            Err(error) => {
                log::warn!("StatusNotifierItem: {error}");
                return;
            }
        };
        while rx.changed().await.is_ok() {
            let model = rx.borrow_and_update().clone();
            if handle
                .update(move |tray| tray.model = model)
                .await
                .is_none()
            {
                break;
            }
        }
        handle.shutdown().await;
    });
    Ok(())
}

/// Register the latest-model channel the D-Bus tray task follows.
pub(crate) fn register<R: Runtime>(
    app: &AppHandle<R>,
    model: TrayModel,
) -> watch::Receiver<TrayModel> {
    let (tx, rx) = watch::channel(model);
    app.manage(LinuxTrayHandle(tx));
    rx
}

pub fn update<R: Runtime>(app: &AppHandle<R>, model: TrayModel) {
    if let Some(handle) = app.try_state::<LinuxTrayHandle>() {
        // Nonblocking, latest-value channel: never wait for D-Bus on GTK's main thread.
        handle.0.send_replace(model);
    }
}

#[cfg(test)]
mod tests;
