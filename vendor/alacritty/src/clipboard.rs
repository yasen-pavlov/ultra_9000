// Modified copy of alacritty/src/clipboard.rs at def47a5dd511ed20607eb5d9e5e7140afd16b31d.
// Changes: macOS always uses the `Default` NSPasteboard provider exactly as upstream does. On
// Linux the terminal shares the GTK host's clipboard and primary selection (`GtkClipboard`)
// instead of upstream's smithay Wayland clipboard: a second `wl_data_device` of the same
// client next to GTK's does not receive selection offers on every compositor (Hyprland sends
// them to one device per client), so pasting found an empty selection. No X11 clipboard
// (Linux support is Wayland only).

use log::{debug, warn};

use alacritty_terminal::term::ClipboardType;

use copypasta::nop_clipboard::NopClipboardContext;
#[cfg(target_os = "macos")]
use copypasta::ClipboardContext;
use copypasta::ClipboardProvider;

pub struct Clipboard {
    clipboard: Box<dyn ClipboardProvider>,
    selection: Option<Box<dyn ClipboardProvider>>,
}

impl Clipboard {
    /// Used for tests, to handle missing clipboard provider when built without the `x11`
    /// feature, and as default clipboard value.
    pub fn new_nop() -> Self {
        Self { clipboard: Box::new(NopClipboardContext::new().unwrap()), selection: None }
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        return Self { clipboard: Box::new(ClipboardContext::new().unwrap()), selection: None };

        #[cfg(target_os = "linux")]
        return gtk_clipboard::clipboard();
    }
}

/// The GTK host's clipboard and primary selection.
///
/// Alacritty reads the clipboard synchronously while it handles a key or mouse event, but GTK
/// only reads another client's selection asynchronously (a synchronous read would spin a
/// nested main loop inside the terminal's event processing). The text of each selection is
/// therefore fetched whenever its owner changes and kept until the next change; pasting
/// returns that text.
#[cfg(target_os = "linux")]
mod gtk_clipboard {
    use std::cell::{Cell, RefCell};
    use std::error::Error;
    use std::rc::Rc;

    use copypasta::ClipboardProvider;
    use gtk::glib::prelude::*;
    use gtk::gdk;

    use super::Clipboard;

    type Result<T, E = Box<dyn Error + Send + Sync + 'static>> = std::result::Result<T, E>;

    /// Text of one selection, refreshed on every owner change.
    struct Cached {
        clipboard: gtk::Clipboard,
        text: RefCell<Option<String>>,
        /// Incremented on every owner change, so a slower earlier fetch cannot overwrite a
        /// newer one.
        generation: Cell<u64>,
    }

    impl Cached {
        fn new(selection: &gdk::Atom) -> Rc<Self> {
            let cached = Rc::new(Self {
                clipboard: gtk::Clipboard::get(selection),
                text: RefCell::new(None),
                generation: Cell::new(0),
            });

            let weak = Rc::downgrade(&cached);
            cached.clipboard.connect_local("owner-change", false, move |_| {
                if let Some(cached) = weak.upgrade() {
                    cached.refresh();
                }
                None
            });
            cached.refresh();
            cached
        }

        /// Fetch the selection's current text.
        fn refresh(self: &Rc<Self>) {
            let generation = self.generation.get() + 1;
            self.generation.set(generation);

            let weak = Rc::downgrade(self);
            self.clipboard.request_text(move |_, text| {
                let Some(cached) = weak.upgrade() else { return };
                if cached.generation.get() == generation {
                    *cached.text.borrow_mut() = text.map(str::to_owned);
                }
            });
        }
    }

    thread_local! {
        /// The clipboard and primary selection, created on first use on the main thread.
        static SELECTIONS: RefCell<Option<(Rc<Cached>, Rc<Cached>)>> = const { RefCell::new(None) };
    }

    /// One selection as an Alacritty clipboard provider.
    ///
    /// `ClipboardProvider` requires `Send`, so the provider names its selection and finds the
    /// main thread's GTK clipboard when used (Alacritty only uses it on the main thread).
    #[derive(Clone, Copy)]
    enum Provider {
        Clipboard,
        Primary,
    }

    impl Provider {
        fn with<T>(self, f: impl FnOnce(&Cached) -> T) -> Result<T> {
            SELECTIONS.with(|cell| {
                let cell = cell.borrow();
                let (clipboard, primary) =
                    cell.as_ref().ok_or("GTK clipboard used off the main thread")?;
                Ok(f(match self {
                    Self::Clipboard => clipboard,
                    Self::Primary => primary,
                }))
            })
        }
    }

    impl ClipboardProvider for Provider {
        fn get_contents(&mut self) -> Result<String> {
            self.with(|cached| cached.text.borrow().clone())?
                .ok_or_else(|| "selection holds no text".into())
        }

        fn set_contents(&mut self, contents: String) -> Result<()> {
            self.with(|cached| {
                cached.clipboard.set_text(&contents);
                // Our own text is current right away: an older fetch still in flight must not
                // overwrite it. The owner change refetches the same text.
                cached.generation.set(cached.generation.get() + 1);
                *cached.text.borrow_mut() = Some(contents);
            })
        }
    }

    /// Clipboard providers sharing the process' GTK clipboard and primary selection.
    pub(super) fn clipboard() -> Clipboard {
        SELECTIONS.with(|cell| {
            cell.borrow_mut().get_or_insert_with(|| {
                (Cached::new(&gdk::SELECTION_CLIPBOARD), Cached::new(&gdk::SELECTION_PRIMARY))
            });
        });

        Clipboard {
            clipboard: Box::new(Provider::Clipboard),
            selection: Some(Box::new(Provider::Primary)),
        }
    }
}

impl Clipboard {
    pub fn store(&mut self, ty: ClipboardType, text: impl Into<String>) {
        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            (ClipboardType::Selection, None) => return,
            _ => &mut self.clipboard,
        };

        clipboard.set_contents(text.into()).unwrap_or_else(|err| {
            warn!("Unable to store text in clipboard: {}", err);
        });
    }

    pub fn load(&mut self, ty: ClipboardType) -> String {
        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            _ => &mut self.clipboard,
        };

        match clipboard.get_contents() {
            Err(err) => {
                debug!("Unable to load text from clipboard: {}", err);
                String::new()
            },
            Ok(text) => text,
        }
    }
}
