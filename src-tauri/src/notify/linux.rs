//! A FIFO sender and a continuously drained listener share one connection. Tracker locks
//! cover only in-memory updates, never D-Bus calls. Receive positions distinguish replaced
//! notifications from stale signals; service owner changes invalidate all server ids.
use std::{
    collections::HashMap,
    sync::{mpsc, Arc, LazyLock, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use zbus::{
    blocking::{connection::Builder, fdo::DBusProxy, Connection, MessageIterator},
    message::{Sequence, Type},
    zvariant::Value,
    MatchRule,
};

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const DBUS: &str = "org.freedesktop.DBus";
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);
type Click = (String, Option<String>);

enum Event {
    Token(String),
    Action(String),
    Closed,
}
struct Shown<P> {
    identifier: String,
    token: Option<String>,
    position: P,
}
// Production uses zbus receive positions; tests inject simple ordered positions.
#[derive(Default)]
struct Tracker<P> {
    shown: HashMap<u32, Shown<P>>,
    latest: HashMap<String, u32>,
    owner: Option<String>,
    owner_position: Option<P>,
    disconnected: bool,
}
impl<P: Copy + Ord> Tracker<P> {
    fn replaces(&self, identifier: &str) -> u32 {
        self.latest.get(identifier).copied().unwrap_or(0)
    }
    fn owner_changed(&mut self, owner: Option<&str>, position: P) -> bool {
        // An older queued NameOwnerChanged must not erase a newer Notify reply, nor
        // may an old reply reintroduce ids after a newer owner-change signal.
        if self.owner_position.is_some_and(|last| position < last) {
            return false;
        }
        self.owner_position = Some(position);
        if self.owner.as_deref() != owner {
            self.shown.clear();
            self.latest.clear();
            self.owner = owner.map(str::to_owned);
        }
        true
    }
    fn shown(&mut self, owner: &str, id: u32, identifier: String, position: P) -> bool {
        if !self.owner_changed(Some(owner), position) {
            return false;
        }
        if let Some(previous) = self.latest.get(&identifier).copied() {
            let _ = self.forget(previous);
        }
        let _ = self.forget(id);
        self.latest.insert(identifier.clone(), id);
        self.shown.insert(
            id,
            Shown {
                identifier,
                token: None,
                position,
            },
        );
        true
    }
    fn forget(&mut self, id: u32) -> Option<Click> {
        let shown = self.shown.remove(&id)?;
        self.latest.remove(&shown.identifier);
        Some((shown.identifier, shown.token))
    }
    fn event(&mut self, owner: &str, id: u32, position: P, event: Event) -> Option<Click> {
        if self.owner.as_deref() != Some(owner) {
            return None;
        }
        let shown = self.shown.get_mut(&id)?;
        if position < shown.position {
            return None;
        }
        match event {
            Event::Token(token) => {
                shown.token = Some(token);
                None
            }
            Event::Action(action) if action == "default" => self.forget(id),
            Event::Closed => {
                let _ = self.forget(id);
                None
            }
            _ => None,
        }
    }
}
struct Bus {
    connection: Connection,
    tracker: Arc<Mutex<Tracker<Sequence>>>,
}
impl Bus {
    fn tracker(&self) -> MutexGuard<'_, Tracker<Sequence>> {
        self.tracker.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn connect(builder: Builder<'_>, on_click: ClickHandler) -> zbus::Result<Self> {
        let connection = builder.method_timeout(METHOD_TIMEOUT).build()?;
        let proxy = DBusProxy::new(&connection)?;
        proxy.add_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(DESTINATION)?
                .interface(DESTINATION)?
                .path(PATH)?
                .build(),
        )?;
        proxy.add_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(DBUS)?
                .interface(DBUS)?
                .path("/org/freedesktop/DBus")?
                .member("NameOwnerChanged")?
                .add_arg(DESTINATION)?
                .build(),
        )?;
        // No announcements exist yet. Create the iterator after subscribing so no
        // bounded queue can fill while the subscription calls wait for their replies.
        let messages = MessageIterator::from(&connection);
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let listener = tracker.clone();
        std::thread::Builder::new()
            .name("u9k-notify-events".into())
            .spawn(move || {
                for message in messages {
                    let Ok(message) = message else { break };
                    let header = message.header();
                    if header.message_type() != Type::Signal {
                        continue;
                    }
                    let mut tracker = listener.lock().unwrap_or_else(PoisonError::into_inner);
                    if header.sender().is_some_and(|s| s.as_str() == DBUS)
                        && header.interface().is_some_and(|i| i.as_str() == DBUS)
                        && header
                            .member()
                            .is_some_and(|m| m.as_str() == "NameOwnerChanged")
                    {
                        if let Ok((name, _, new)) =
                            message.body().deserialize::<(String, String, String)>()
                        {
                            if name == DESTINATION {
                                tracker.owner_changed(
                                    (!new.is_empty()).then_some(new.as_str()),
                                    message.recv_position(),
                                );
                            }
                        }
                        continue;
                    }
                    let Some(owner) = header.sender() else {
                        continue;
                    };
                    if !header
                        .interface()
                        .is_some_and(|i| i.as_str() == DESTINATION)
                        || !header.path().is_some_and(|p| p.as_str() == PATH)
                    {
                        continue;
                    }
                    let body = message.body();
                    let event = match header.member().map(|m| m.as_str()) {
                        Some("ActivationToken") => body
                            .deserialize::<(u32, String)>()
                            .ok()
                            .map(|(id, token)| (id, Event::Token(token))),
                        Some("ActionInvoked") => body
                            .deserialize::<(u32, String)>()
                            .ok()
                            .map(|(id, action)| (id, Event::Action(action))),
                        Some("NotificationClosed") => body
                            .deserialize::<(u32, u32)>()
                            .ok()
                            .map(|(id, _)| (id, Event::Closed)),
                        _ => None,
                    };
                    let clicked = event.and_then(|(id, event)| {
                        tracker.event(owner.as_str(), id, message.recv_position(), event)
                    });
                    drop(tracker);
                    if let Some((identifier, token)) = clicked {
                        on_click(&identifier, token);
                    }
                }
                let mut tracker = listener.lock().unwrap_or_else(PoisonError::into_inner);
                tracker.disconnected = true;
                tracker.shown.clear();
                tracker.latest.clear();
            })?;
        Ok(Self {
            connection,
            tracker,
        })
    }
    fn notify(&self, notification: Notification) -> zbus::Result<()> {
        let replaces = self.tracker().replaces(&notification.identifier);
        let hints = HashMap::from([("desktop-entry", Value::from("ultra9000"))]);
        // The well-known destination preserves normal D-Bus service activation.
        let reply = self.connection.call_method(
            Some(DESTINATION),
            PATH,
            Some(DESTINATION),
            "Notify",
            &(
                "Ultra 9000",
                replaces,
                "",
                notification.summary,
                notification.body,
                vec!["default", "Show"],
                hints,
                -1i32,
            ),
        )?;
        let id = reply.body().deserialize::<u32>()?;
        let header = reply.header();
        let owner = header
            .sender()
            .ok_or_else(|| zbus::Error::Failure("notification reply has no sender".into()))?;
        if !self.tracker().shown(
            owner.as_str(),
            id,
            notification.identifier,
            reply.recv_position(),
        ) {
            return Err(zbus::Error::Failure(
                "notification server changed before its reply was registered".into(),
            ));
        }
        Ok(())
    }
}

struct Notification {
    identifier: String,
    summary: String,
    body: String,
}
type ClickHandler = Arc<dyn Fn(&str, Option<String>) + Send + Sync>;

fn worker(
    connect: impl Fn() -> zbus::Result<Builder<'static>> + Send + 'static,
    clicked: ClickHandler,
) -> Option<mpsc::Sender<Notification>> {
    let (sender, requests) = mpsc::channel();
    std::thread::Builder::new()
        .name("u9k-notify".into())
        .spawn(move || {
            let mut bus: Option<Bus> = None;
            for notification in requests {
                if bus.as_ref().is_some_and(|b| b.tracker().disconnected) {
                    bus = None;
                }
                let result = (|| {
                    if bus.is_none() {
                        bus = Some(Bus::connect(connect()?, clicked.clone())?);
                    }
                    bus.as_ref().expect("connected above").notify(notification)
                })();
                if let Err(error) = result {
                    log::warn!("desktop notification failed: {error}");
                }
            }
        })
        .inspect_err(|error| log::warn!("notification worker failed: {error}"))
        .ok()
        .map(|_| sender)
}
static WORKER: LazyLock<Option<mpsc::Sender<Notification>>> = LazyLock::new(|| {
    worker(
        Builder::session,
        Arc::new(|identifier, token| {
            super::clicked(crate::workspace::app_handle, identifier, token);
        }),
    )
});
pub(super) fn send(item: &str, summary: &str, body: &str) {
    if let Some(worker) = &*WORKER {
        enqueue(worker, item, summary, body);
    }
}

fn enqueue(worker: &mpsc::Sender<Notification>, item: &str, summary: &str, body: &str) {
    if let Err(error) = worker.send(Notification {
        identifier: super::identifier(item),
        summary: summary.to_owned(),
        body: body.to_owned(),
    }) {
        log::warn!("notification worker unavailable: {error}");
    }
}

#[cfg(test)]
mod tests;
