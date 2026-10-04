//! Keys: moving, opening and leaving screens, filtering and toggles.

use super::{App, AppRow, Data, Frame, Key, Screen, TABS, WINDOWS};
use crate::api::Request;
use crate::apps::Sort;
use jiff::Timestamp;
use skym_core::subject::{AppKey, Subject};

impl App {
    fn open(&mut self, screen: Screen) -> Vec<Request> {
        match &screen {
            Screen::Host(_) => self.host = Data::default(),
            Screen::Workload(_) => self.workload = Data::default(),
            Screen::Exceptions(_) => self.exceptions = Data::default(),
            Screen::Timeline { .. } => self.timeline = Data::default(),
            Screen::App(_) => self.app = Data::default(),
            Screen::Problems | Screen::Apps | Screen::Hosts => {}
        }
        let request = screen.request();
        self.filter = None;
        self.stack.push(Frame { screen, cursor: 0, expanded: None });
        request.into_iter().collect()
    }

    fn enter(&mut self, now: Timestamp) -> Vec<Request> {
        let cursor = self.frame().cursor;
        let target = match &self.frame().screen {
            // Where the problem belongs: its service, its application, or its host.
            Screen::Problems => self.problem_rows(now).get(cursor).and_then(|r| {
                match (&r.incident.subject, r.app()) {
                    (Subject::Workload(k), _) => Some(Screen::Workload(k.clone())),
                    (Subject::Host(h) | Subject::Mount { host: h, .. }, _) => {
                        Some(Screen::Host(h.clone()))
                    }
                    (_, Some(app)) => Some(Screen::App(app.clone())),
                    (_, None) => None,
                }
            }),
            Screen::Apps => match self.app_rows().into_iter().nth(cursor) {
                Some(AppRow::App(a)) => Some(Screen::App(a.key.clone())),
                Some(AppRow::Group(g)) => {
                    let group = (self.grouping, g.name);
                    if !self.open_groups.remove(&group) {
                        self.open_groups.insert(group);
                    }
                    None
                }
                None => None,
            },
            Screen::Hosts => self.host_rows().get(cursor).map(|h| Screen::Host(h.id.clone())),
            Screen::Host(_) => self.host_apps().get(cursor).map(|a| Screen::App(a.key.clone())),
            Screen::App(_) => self.services().get(cursor).map(|w| Screen::Workload(w.key.clone())),
            Screen::Workload(_) | Screen::Exceptions(_) => {
                let frame = self.frame_mut();
                frame.expanded = (frame.expanded != Some(cursor)).then_some(cursor);
                None
            }
            Screen::Timeline { .. } => None,
        };
        target.map_or_else(Vec::new, |screen| self.open(screen))
    }

    /// The application on each row of the applications tab or a host's page (`None`: a
    /// group's header).
    fn app_keys(&self) -> Vec<Option<AppKey>> {
        match self.frame().screen {
            Screen::Apps => self
                .app_rows()
                .into_iter()
                .map(|r| match r {
                    AppRow::App(a) => Some(a.key.clone()),
                    AppRow::Group(_) => None,
                })
                .collect(),
            _ => self.host_apps().iter().map(|a| Some(a.key.clone())).collect(),
        }
    }

    /// Sorts the other way, the selection staying on its application (a header stays put).
    fn toggle_sort(&mut self) {
        let selected = self.app_keys().get(self.frame().cursor).cloned().flatten();
        self.sort = match self.sort {
            Sort::Problems => Sort::Memory,
            Sort::Memory => Sort::Problems,
        };
        let at = selected.and_then(|k| self.app_keys().iter().position(|r| r.as_ref() == Some(&k)));
        if let Some(at) = at {
            self.frame_mut().cursor = at;
        }
    }

    /// Opens every group of the current grouping, or folds them all when all are open.
    fn open_or_fold_all(&mut self) {
        let names: Vec<String> = self
            .app_rows()
            .into_iter()
            .filter_map(|r| match r {
                AppRow::Group(g) => Some(g.name),
                AppRow::App(_) => None,
            })
            .collect();
        let by = self.grouping;
        if names.iter().all(|n| self.open_groups.contains(&(by, n.clone()))) {
            self.open_groups.retain(|(g, _)| *g != by);
        } else {
            self.open_groups.extend(names.into_iter().map(|n| (by, n)));
        }
        self.frame_mut().cursor = 0;
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
            Screen::Problems | Screen::Apps | Screen::Hosts => false,
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
            Screen::Problems | Screen::Apps | Screen::Hosts => {}
        }
        self.frame().screen.request().into_iter().collect()
    }

    /// The next tab, as the new root: what was open on the previous one is left.
    fn next_tab(&mut self) -> Vec<Request> {
        let at = TABS.iter().position(|t| t == self.tab()).unwrap_or(0);
        let screen = TABS[(at + 1) % TABS.len()].clone();
        // Problems and hosts come with the overview; only the apps tab may not be read yet.
        let request = screen.request().filter(|_| self.apps.value.is_none());
        self.stack = vec![Frame { screen, cursor: 0, expanded: None }];
        self.filter = None;
        request.into_iter().collect()
    }

    fn step(&mut self, down: bool, now: Timestamp) {
        let rows = self.rows(now);
        let cursor = &mut self.frame_mut().cursor;
        *cursor = match down {
            true => (*cursor + 1).min(rows.saturating_sub(1)),
            false => cursor.saturating_sub(1),
        };
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
            return Vec::new();
        }
        match key {
            Key::Char('q') => self.quit = true,
            Key::Char('?') => self.help = true,
            Key::Char('r') => return self.refresh(now),
            Key::Char('/') => (self.filter, self.editing) = (Some(String::new()), true),
            Key::Char('h') => self.show_info = !self.show_info,
            Key::Char('p') => self.preview = !self.preview,
            Key::Char('s') if matches!(self.frame().screen, Screen::Apps | Screen::Host(_)) => {
                self.toggle_sort();
            }
            Key::Char('m') => {
                self.show_muted = !self.show_muted;
                if self.show_muted {
                    return vec![Request::Muted];
                }
            }
            Key::Char('t') => return self.timeline(),
            Key::Char('e') => match &self.frame().screen {
                Screen::Host(h) => return self.open(Screen::Exceptions(h.clone())),
                Screen::Apps => self.open_or_fold_all(),
                _ => {}
            },
            Key::Char('g') if self.frame().screen == Screen::Apps => {
                (self.grouping, self.frame_mut().cursor) = (self.grouping.next(), 0);
            }
            Key::Char('[') => return self.shift_window(-1),
            Key::Char(']') => return self.shift_window(1),
            Key::Esc | Key::Backspace => return self.back(),
            Key::Tab => return self.next_tab(),
            Key::Down | Key::Char('j') => self.step(true, now),
            Key::Up | Key::Char('k') => self.step(false, now),
            Key::Enter => return self.enter(now),
            _ => {}
        }
        Vec::new()
    }
}
