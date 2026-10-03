//! What the view shows and how keys and answers change it. Pure: `update` returns the
//! requests to make, and `main` makes them.

use crate::api::{FetchError, Payload, Request};
use crate::problems::{Problems, Row, problems};
use jiff::{SignedDuration, Timestamp};
use skym_core::subject::{HostId, Subject, WorkloadKey};
use skym_core::view::{
    ExceptionList, HostView, IncidentList, Overview, Status, Timeline, WorkloadSummary,
    WorkloadView,
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
    pub pane: Pane,
    /// 0 is "All hosts", then `host_ids()` in order.
    pub host_cursor: usize,
    pub filter: Option<String>,
    pub editing: bool,
    pub show_info: bool,
    pub show_muted: bool,
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
            pane: Pane::Hosts,
            host_cursor: 0,
            filter: None,
            editing: false,
            show_info: false,
            show_muted: false,
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

    /// Hosts in the overview's order: by customer, most urgent first.
    pub fn host_ids(&self) -> Vec<&HostId> {
        let customers = self.overview.value.iter().flat_map(|o| &o.customers);
        let ids = customers.flat_map(|c| &c.hosts).map(|h| &h.id);
        ids.filter(|id| !self.filtering(Pane::Hosts) || self.matches(id)).collect()
    }

    /// When skym started watching a host, from the overview.
    pub fn observed_since(&self, host: &str) -> Option<Timestamp> {
        let customers = self.overview.value.iter().flat_map(|o| &o.customers);
        customers.flat_map(|c| &c.hosts).find(|h| h.id == host).and_then(|h| h.observed_since)
    }

    /// The filter applies to the list in view: on the overview, the focused pane's.
    fn filtering(&self, pane: Pane) -> bool {
        self.frame().screen == Screen::Overview && self.pane == pane
    }

    /// The host picked in the overview's left pane, if not "All hosts".
    pub fn picked_host(&self) -> Option<&HostId> {
        self.host_cursor.checked_sub(1).and_then(|i| self.host_ids().get(i).copied())
    }

    /// The overview's problems for the picked host, filtered by name.
    pub fn problem_groups(&self, now: Timestamp) -> Problems<'_> {
        let Some(overview) = &self.overview.value else { return Problems::default() };
        let muted = match (&self.muted.value, self.show_muted) {
            (Some(list), true) => list.incidents.as_slice(),
            _ => &[],
        };
        let mut p = problems(overview, muted, self.picked_host().map(String::as_str), now);
        if self.filtering(Pane::Problems) {
            for group in [&mut p.new, &mut p.ongoing, &mut p.info] {
                group
                    .retain(|r| self.matches(&format!("{} {}", r.host, name(&r.incident.subject))));
            }
        }
        p
    }

    /// The overview's problem rows as listed: new, ongoing, then hygiene when shown.
    pub fn problem_rows(&self, now: Timestamp) -> Vec<Row<'_>> {
        let p = self.problem_groups(now);
        let info = if self.show_info { p.info } else { Vec::new() };
        p.new.into_iter().chain(p.ongoing).chain(info).collect()
    }

    /// A host's services: those with problems first, then by name.
    pub fn services(&self) -> Vec<&WorkloadSummary> {
        let mut list: Vec<&WorkloadSummary> = self
            .host
            .value
            .iter()
            .flat_map(|h| &h.workloads)
            .filter(|w| self.matches(&format!("{}/{}", w.key.project, w.key.service)))
            .collect();
        list.sort_by(|a, b| {
            let worse = |w: &WorkloadSummary| w.status != Status::Ok;
            worse(b).cmp(&worse(a)).then(b.status.cmp(&a.status)).then(a.key.cmp(&b.key))
        });
        list
    }

    /// Keeps the picked host within the host list as it shrinks.
    fn clamp_host_cursor(&mut self) {
        self.host_cursor = self.host_cursor.min(self.host_ids().len());
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| name.to_lowercase().contains(&f.to_lowercase()))
    }

    /// How many rows the current list has (for moving the selection).
    fn rows(&self, now: Timestamp) -> usize {
        match (&self.frame().screen, self.pane) {
            (Screen::Overview, Pane::Hosts) => self.host_ids().len() + 1,
            (Screen::Overview, Pane::Problems) => self.problem_rows(now).len(),
            (Screen::Host(_), _) => self.services().len(),
            (Screen::Workload(_), _) => {
                self.workload.value.as_ref().map_or(0, |w| w.exceptions.len())
            }
            (Screen::Exceptions(_), _) => {
                self.exceptions.value.as_ref().map_or(0, |e| e.exceptions.len())
            }
            (Screen::Timeline { .. }, _) => {
                self.timeline.value.as_ref().map_or(0, |t| t.entries.len())
            }
        }
    }

    fn refresh(&mut self, now: Timestamp) -> Vec<Request> {
        self.last_refresh = Some(now);
        let muted = self.show_muted.then_some(Request::Muted);
        [Some(Request::Overview), muted, self.frame().screen.request()]
            .into_iter()
            .flatten()
            .collect()
    }

    fn open(&mut self, screen: Screen) -> Vec<Request> {
        match &screen {
            Screen::Host(_) => self.host = Data::default(),
            Screen::Workload(_) => self.workload = Data::default(),
            Screen::Exceptions(_) => self.exceptions = Data::default(),
            Screen::Timeline { .. } => self.timeline = Data::default(),
            Screen::Overview => {}
        }
        let request = screen.request();
        self.filter = None;
        self.stack.push(Frame { screen, cursor: 0, expanded: None });
        request.into_iter().collect()
    }

    fn enter(&mut self, now: Timestamp) -> Vec<Request> {
        let cursor = self.frame().cursor;
        let target = match (&self.frame().screen, self.pane) {
            (Screen::Overview, Pane::Hosts) => self.picked_host().cloned().map(Screen::Host),
            (Screen::Overview, Pane::Problems) => {
                self.problem_rows(now).get(cursor).map(|r| match &r.incident.subject {
                    Subject::Workload(k) => Screen::Workload(k.clone()),
                    _ => Screen::Host(r.host.to_string()),
                })
            }
            (Screen::Host(_), _) => {
                self.services().get(cursor).map(|w| Screen::Workload(w.key.clone()))
            }
            (Screen::Workload(_) | Screen::Exceptions(_), _) => {
                let frame = self.frame_mut();
                frame.expanded = (frame.expanded != Some(cursor)).then_some(cursor);
                None
            }
            (Screen::Timeline { .. }, _) => None,
        };
        target.map_or_else(Vec::new, |screen| self.open(screen))
    }

    /// From a host or a service; a timeline has no timeline of its own.
    fn timeline(&mut self) -> Vec<Request> {
        let (host, workload) = match &self.frame().screen {
            Screen::Host(h) => (h.clone(), None),
            Screen::Workload(k) => (k.host.clone(), Some(k.clone())),
            _ => return Vec::new(),
        };
        self.open(Screen::Timeline { host, workload, window: 1 })
    }

    fn shift_window(&mut self, by: isize) -> Vec<Request> {
        let Screen::Timeline { window, .. } = &mut self.frame_mut().screen else {
            return Vec::new();
        };
        let next = window.saturating_add_signed(by).min(WINDOWS.len() - 1);
        if next == *window {
            return Vec::new();
        }
        *window = next;
        self.timeline = Data::default();
        self.frame().screen.request().into_iter().collect()
    }

    fn back(&mut self) {
        if self.filter.take().is_none() && self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    fn step(&mut self, down: bool, now: Timestamp) {
        let rows = self.rows(now);
        let cursor = match (&self.frame().screen, self.pane) {
            (Screen::Overview, Pane::Hosts) => &mut self.host_cursor,
            _ => &mut self.frame_mut().cursor,
        };
        *cursor = match down {
            true => (*cursor + 1).min(rows.saturating_sub(1)),
            false => cursor.saturating_sub(1),
        };
        if self.frame().screen == Screen::Overview && self.pane == Pane::Hosts {
            self.frame_mut().cursor = 0; // another host's problems
        }
    }

    fn key(&mut self, key: Key, now: Timestamp) -> Vec<Request> {
        if key == Key::Quit {
            self.quit = true;
            return Vec::new();
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        if self.editing {
            match key {
                Key::Char(c) => self.filter.get_or_insert_default().push(c),
                Key::Backspace => drop(self.filter.as_mut().and_then(String::pop)),
                Key::Enter => self.editing = false,
                Key::Esc => (self.filter, self.editing) = (None, false),
                _ => {}
            }
            self.frame_mut().cursor = 0;
            self.clamp_host_cursor();
            return Vec::new();
        }
        match key {
            Key::Char('q') => self.quit = true,
            Key::Char('?') => self.help = true,
            Key::Char('r') => return self.refresh(now),
            Key::Char('/') => (self.filter, self.editing) = (Some(String::new()), true),
            Key::Char('h') => self.show_info = !self.show_info,
            Key::Char('m') => {
                self.show_muted = !self.show_muted;
                if self.show_muted {
                    return vec![Request::Muted];
                }
            }
            Key::Char('t') => return self.timeline(),
            Key::Char('e') => {
                if let Screen::Host(h) = &self.frame().screen {
                    return self.open(Screen::Exceptions(h.clone()));
                }
            }
            Key::Char('[') => return self.shift_window(-1),
            Key::Char(']') => return self.shift_window(1),
            Key::Esc | Key::Backspace => {
                self.back();
                self.clamp_host_cursor();
            }
            Key::Tab if self.frame().screen == Screen::Overview => {
                self.pane = match self.pane {
                    Pane::Hosts => Pane::Problems,
                    Pane::Problems => Pane::Hosts,
                };
            }
            Key::Down | Key::Char('j') => self.step(true, now),
            Key::Up | Key::Char('k') => self.step(false, now),
            Key::Enter => return self.enter(now),
            _ => {}
        }
        Vec::new()
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

/// A subject's name as the lists show it: `project/service`, or the host's own part.
fn name(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) => format!("{}/{}", k.project, k.service),
        Subject::Mount { path, .. } => path.clone(),
        Subject::Host(_) => "host".into(),
        Subject::Endpoint(url) => url.clone(),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
