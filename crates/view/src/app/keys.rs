//! Keys: moving, opening and leaving screens, filtering and toggles.

use super::{App, Data, Frame, Key, Pane, Screen, WINDOWS};
use crate::api::Request;
use jiff::Timestamp;
use skym_core::subject::Subject;

impl App {
    fn open(&mut self, screen: Screen) -> Vec<Request> {
        match &screen {
            Screen::Host(_) => self.host = Data::default(),
            Screen::Workload(_) => self.workload = Data::default(),
            Screen::Exceptions(_) => self.exceptions = Data::default(),
            Screen::Timeline { .. } => self.timeline = Data::default(),
            Screen::Apps => self.apps = Data::default(),
            Screen::App(_) => self.app = Data::default(),
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
            (Screen::Overview, Pane::Hosts) => match self.picked() {
                Some(Subject::Host(h)) => Some(Screen::Host(h)),
                _ => None, // an endpoint has no screen of its own
            },
            (Screen::Overview, Pane::Problems) => {
                self.problem_rows(now).get(cursor).and_then(|r| match &r.incident.subject {
                    Subject::Workload(k) => Some(Screen::Workload(k.clone())),
                    Subject::Endpoint(_) => None,
                    _ => Some(Screen::Host(r.host.to_string())),
                })
            }
            (Screen::Host(_) | Screen::App(_), _) => {
                self.services().get(cursor).map(|w| Screen::Workload(w.key.clone()))
            }
            (Screen::Apps, _) => self.app_rows().get(cursor).map(|a| Screen::App(a.key.clone())),
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

    /// Clears the filter, or returns to the screen below, reading it again if what is held
    /// now belongs to another host, service or application (or cannot tell).
    fn back(&mut self) -> Vec<Request> {
        if self.filter.take().is_some() || self.stack.len() == 1 {
            return Vec::new();
        }
        self.stack.pop();
        let stale = match &self.frame().screen {
            Screen::Overview | Screen::Apps => false,
            // Nothing held (its answer was dropped while away, or failed) is read again too.
            Screen::Host(h) => self.host.value.as_ref().is_none_or(|v| v.id != *h),
            Screen::Workload(k) => self.workload.value.as_ref().is_none_or(|v| v.key != *k),
            Screen::App(a) => self.app.value.as_ref().is_none_or(|v| v.app.key != *a),
            Screen::Exceptions(_) | Screen::Timeline { .. } => true,
        };
        if !stale {
            return Vec::new();
        }
        match &self.frame().screen {
            Screen::Host(_) => self.host = Data::default(),
            Screen::Workload(_) => self.workload = Data::default(),
            Screen::App(_) => self.app = Data::default(),
            Screen::Exceptions(_) => self.exceptions = Data::default(),
            Screen::Timeline { .. } => self.timeline = Data::default(),
            Screen::Overview | Screen::Apps => {}
        }
        self.frame().screen.request().into_iter().collect()
    }

    /// The applications page, where it already is in the stack, else opened.
    fn apps(&mut self) -> Vec<Request> {
        match self.stack.iter().position(|f| f.screen == Screen::Apps) {
            Some(i) => {
                self.stack.truncate(i + 1);
                self.filter = None;
                vec![Request::Apps]
            }
            None => self.open(Screen::Apps),
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

    pub(super) fn key(&mut self, key: Key, now: Timestamp) -> Vec<Request> {
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
            Key::Char('e') => match &self.frame().screen {
                Screen::Host(h) => return self.open(Screen::Exceptions(h.clone())),
                Screen::Apps => (self.all_envs, self.frame_mut().cursor) = (!self.all_envs, 0),
                _ => {}
            },
            Key::Char('a') if self.frame().screen != Screen::Apps => return self.apps(),
            Key::Char('[') => return self.shift_window(-1),
            Key::Char(']') => return self.shift_window(1),
            Key::Esc | Key::Backspace => {
                let requests = self.back();
                self.clamp_host_cursor();
                return requests;
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
}
