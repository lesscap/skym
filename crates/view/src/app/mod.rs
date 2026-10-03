//! What the view shows and how keys and answers change it. Pure: `update` returns the
//! requests to make, and `main` makes them.

use crate::api::{FetchError, Payload, Request};
use jiff::{SignedDuration, Timestamp};
use skym_core::subject::{AppKey, HostId, WorkloadKey};
use skym_core::view::{
    AppList, AppView, ExceptionList, HostView, IncidentList, Overview, Timeline, WorkloadView,
};

pub const REFRESH: SignedDuration = SignedDuration::from_secs(30);
pub const WINDOWS: [&str; 3] = ["6h", "24h", "7d"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Tab,
    Esc,
    Backspace,
    Char(char),
    /// Ctrl-C: quits from anywhere, even while typing a filter.
    Quit,
}

#[derive(Debug)]
pub enum Msg {
    Key(Key),
    /// Once a second: time moves on, and every `REFRESH` the data is read again.
    Tick,
    Fetched(Request, Result<Box<Payload>, FetchError>),
    /// The view cannot go on (the terminal stopped answering).
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Host(HostId),
    Workload(WorkloadKey),
    Exceptions(HostId),
    /// `window` indexes `WINDOWS`.
    Timeline {
        host: HostId,
        workload: Option<WorkloadKey>,
        window: usize,
    },
    Apps,
    App(AppKey),
}

impl Screen {
    fn request(&self) -> Option<Request> {
        match self {
            Screen::Overview => None,
            Screen::Host(h) => Some(Request::Host(h.clone())),
            Screen::Workload(k) => Some(Request::Workload(k.clone())),
            Screen::Exceptions(h) => Some(Request::Exceptions(h.clone())),
            Screen::Timeline { host, workload, window } => Some(Request::Timeline {
                host: host.clone(),
                workload: workload.clone(),
                since: WINDOWS[*window],
            }),
            Screen::Apps => Some(Request::Apps),
            Screen::App(a) => Some(Request::App(a.clone())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Hosts,
    Problems,
}

/// The last good answer and when it came.
#[derive(Debug)]
pub struct Data<T> {
    pub value: Option<T>,
    pub at: Option<Timestamp>,
}

impl<T> Default for Data<T> {
    fn default() -> Self {
        Data { value: None, at: None }
    }
}

impl<T> Data<T> {
    fn set(&mut self, value: T, now: Timestamp) {
        (self.value, self.at) = (Some(value), Some(now));
    }
}

/// A screen with its own selection: going back restores where you were.
#[derive(Debug)]
pub struct Frame {
    pub screen: Screen,
    pub cursor: usize,
    /// The expanded row (an exception's stack), if any.
    pub expanded: Option<usize>,
}

#[derive(Debug)]
pub struct App {
    pub stack: Vec<Frame>,
    pub overview: Data<Overview>,
    pub muted: Data<IncidentList>,
    pub host: Data<HostView>,
    pub workload: Data<WorkloadView>,
    pub exceptions: Data<ExceptionList>,
    pub timeline: Data<Timeline>,
    pub apps: Data<AppList>,
    pub app: Data<AppView>,
    pub pane: Pane,
    /// 0 is "All hosts", then `targets()` in order.
    pub host_cursor: usize,
    pub filter: Option<String>,
    pub editing: bool,
    pub show_info: bool,
    pub show_muted: bool,
    /// On the applications page: every environment unfolded.
    pub all_envs: bool,
    pub help: bool,
    /// The last failure; the data stays as it was.
    pub error: Option<FetchError>,
    pub last_refresh: Option<Timestamp>,
    pub quit: bool,
    /// Why the view had to stop (a rejected token).
    pub fatal: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        App {
            stack: vec![Frame { screen: Screen::Overview, cursor: 0, expanded: None }],
            overview: Data::default(),
            muted: Data::default(),
            host: Data::default(),
            workload: Data::default(),
            exceptions: Data::default(),
            timeline: Data::default(),
            apps: Data::default(),
            app: Data::default(),
            pane: Pane::Hosts,
            host_cursor: 0,
            filter: None,
            editing: false,
            show_info: false,
            show_muted: false,
            all_envs: false,
            help: false,
            error: None,
            last_refresh: None,
            quit: false,
            fatal: None,
        }
    }
}

impl App {
    pub fn frame(&self) -> &Frame {
        self.stack.last().expect("the overview is never popped")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.stack.last_mut().expect("the overview is never popped")
    }

    fn refresh(&mut self, now: Timestamp) -> Vec<Request> {
        self.last_refresh = Some(now);
        let muted = self.show_muted.then_some(Request::Muted);
        [Some(Request::Overview), muted, self.frame().screen.request()]
            .into_iter()
            .flatten()
            .collect()
    }

    fn fetched(
        &mut self,
        request: Request,
        result: Result<Box<Payload>, FetchError>,
        now: Timestamp,
    ) {
        if result.as_ref().err() == Some(&FetchError::Unauthorized) {
            self.fatal = Some("the server does not accept this reader token (SKYM_TOKEN)".into());
            self.quit = true;
            return;
        }
        // An answer for a screen no longer in view is dropped, failures included.
        let wanted = request == Request::Overview
            || request == Request::Muted
            || self.frame().screen.request().as_ref() == Some(&request);
        if !wanted {
            return;
        }
        let payload = match result {
            Ok(payload) => payload,
            Err(e) => return self.error = Some(e),
        };
        self.error = None;
        match *payload {
            Payload::Overview(v) => {
                self.overview.set(v, now);
                self.clamp_host_cursor();
            }
            Payload::Muted(v) => self.muted.set(v, now),
            Payload::Host(v) => self.host.set(v, now),
            Payload::Workload(v) => self.workload.set(v, now),
            Payload::Exceptions(v) => self.exceptions.set(v, now),
            Payload::Timeline(v) => self.timeline.set(v, now),
            Payload::Apps(v) => self.apps.set(v, now),
            Payload::App(v) => self.app.set(v, now),
        }
    }
}

/// The view's whole behaviour: a message in, the requests to make out.
pub fn update(app: &mut App, msg: Msg, now: Timestamp) -> Vec<Request> {
    match msg {
        Msg::Tick if app.last_refresh.is_none_or(|t| now.duration_since(t) >= REFRESH) => {
            app.refresh(now)
        }
        Msg::Tick => Vec::new(),
        Msg::Key(key) => app.key(key, now),
        Msg::Fetched(request, result) => {
            app.fetched(request, result, now);
            Vec::new()
        }
        Msg::Failed(why) => {
            (app.fatal, app.quit) = (Some(why), true);
            Vec::new()
        }
    }
}

mod keys;
mod lists;

#[cfg(test)]
mod tests;
