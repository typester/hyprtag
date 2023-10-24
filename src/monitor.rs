use anyhow::bail;
use tokio::{runtime::Handle, sync::mpsc};
use tracing::instrument::WithSubscriber;

use crate::{state::{State, Changes as MonitorChanges}, hyprctl::{MonitorInfo, hyprctl_monitors, hyprctl_batch}, Ctrl};

#[derive(Debug, Clone)]
pub struct Monitor {
    pub id: u8,
    pub name: String,
    state: State,
}

#[derive(Debug)]
pub struct MonitorsState {
    monitors: Vec<Monitor>,
    active_monitor_index: usize,
}

#[derive(Debug)]
pub struct Changes {
    pub active_monitor_index: usize,
    pub changes: MonitorChanges,
}

impl From<Vec<MonitorInfo>> for MonitorsState {
    fn from(value: Vec<MonitorInfo>) -> Self {
        let focused = value.iter().enumerate().find_map(|(i, m)| {
            if m.focused {
                Some(i)
            } else {
                None
            }
        }).unwrap_or(0);

        let monitors = value.iter().map(|m| {
            Monitor {
                id: m.id,
                name: m.name.clone(),
                state: State::new(),
            }
        }).collect();

        Self {
            monitors,
            active_monitor_index: focused,
        }
    }
}

impl MonitorsState {
    pub fn debug_dump(&self) -> String {
        let mut s = format!("Active monitor: {}\n", self.active_monitor_index);
        for monitor in self.monitors.iter() {
            s += format!("Monitor {},{}:\n", monitor.id, monitor.name).as_str();
            s += monitor.state.debug_dump().as_str();
        }
        s
    }

    pub fn next_monitor(&self) -> u8 {
        let next_index = self.active_monitor_index + 1;
        if next_index < self.monitors.len() {
            next_index as u8
        } else {
            0
        }
    }

    pub fn focused_monitor_changed(&mut self, name: &str) -> anyhow::Result<()> {
        let index = self.monitors.iter().enumerate().find_map(|(i, m)| {
            if m.name == name {
                Some(i)
            } else {
                None
            }
        });

        match index {
            Some(index) => {
                self.active_monitor_index = index;
                Ok(())
            },

            None =>  bail!("no such monitor:{}", name)
        }
    }

    pub fn focused_monitor_changed_by_num(&mut self, n: u8) {
        let index = n - 1;
        unimplemented!()
    }

    pub fn new_window_added(&mut self, window: String) -> anyhow::Result<()> {
        tracing::debug!(?window, "new_window_added");
        for (i, monitor) in self.monitors.iter().enumerate() {
            if i == self.active_monitor_index {
                continue;
            }

            if let Some(_) = monitor.state.find_window_tag_index(&window) {
                bail!("window:{} is already in other tag", window);
            }
        }
        self.monitors[self.active_monitor_index].state.new_window_added(window)
    }

    pub fn window_removed(&mut self, window: String) -> anyhow::Result<()> {
        self.monitors[self.active_monitor_index].state.window_removed(window)
    }

    pub fn move_window_to_monitor(&mut self, dest_monitor: u8, window: Option<String>) -> anyhow::Result<()> {
        let window = window.or_else(|| {
            self.monitors[self.active_monitor_index].state.active_window()
        });
        let window = match window {
            Some(w) => w,
            None => bail!("Couldn't detect window"),
        };

        tracing::debug!(%window, %dest_monitor, "move_window_to_monitor");

        let window_removed = self.monitors.iter_mut().find_map(|m| {
            match m.state.window_removed(window.clone()) {
                Ok(_) => Some(true),
                Err(_) => None,
            }
        });

        if window_removed.is_some() {
            self.monitors[dest_monitor as usize].state.new_window_added(window)
        } else {
            bail!("no such window: {}", window)
        }
    }

    pub fn focus_window_changed(&mut self, window: String) -> anyhow::Result<()> {
        let new_window = self.monitors.iter().find(|m| {
            m.state.find_window_tag_index(&window).is_some()
        }).is_none();

        self.monitors[self.active_monitor_index].state.focus_window_changed(window, new_window)
    }

    pub fn move_window(&mut self, dest_tag: u8, window: Option<String>) -> anyhow::Result<Changes> {
        let changes = self.monitors[self.active_monitor_index].state.move_window(dest_tag, window)?;
        Ok(Changes {
            active_monitor_index: self.active_monitor_index,
            changes,
        })
    }

    pub fn set_visible_tags(&mut self, tags: u32) -> anyhow::Result<Changes> {
        let changes = self.monitors[self.active_monitor_index].state.set_visible_tags(tags)?;
        Ok(Changes {
            active_monitor_index: self.active_monitor_index,
            changes,
        })
    }

    pub fn toggle_tag(&mut self, tag: u8) -> anyhow::Result<Changes> {
        let changes = self.monitors[self.active_monitor_index].state.toggle_tag(tag)?;
        Ok(Changes {
            active_monitor_index: self.active_monitor_index,
            changes,
        })
    }

    pub fn restore_prev_tags(&mut self) -> anyhow::Result<Changes> {
        let changes = self.monitors[self.active_monitor_index].state.restore_prev_tags()?;
        Ok(Changes {
            active_monitor_index: self.active_monitor_index,
            changes,
        })
    }

    pub fn monitor_removed(&mut self, name: &str) -> anyhow::Result<(usize, usize, Vec<String>)> {
        let (removed_index, monitor) = match self.monitors.iter().enumerate().find(|(_, m)| m.name == name) {
            Some(m) => m,
            None => bail!("No such monitor: {}", name),
        };

        let (index, first_monitor) = match self.monitors.iter().enumerate().find(|(_, m)| m.name != name) {
            Some(m) => m,
            None => bail!("All monitors were removed?"), // TODO: care this case
        };
        let first_monitor = first_monitor.clone();

        let windows = monitor.state.all_window_addrs();
        for w in windows.iter() {
            self.move_window_to_monitor(first_monitor.id, Some(w.clone()))?;
        }

        self.monitors.remove(removed_index);

        Ok((index, first_monitor.state.active_tag_index(), windows))
    }

    pub(crate) fn monitor_added(&mut self, name: &str, tx: mpsc::Sender<Ctrl>) -> anyhow::Result<()> {
        if let Some(_) = self.monitors.iter().find(|m| m.name == name) {
            bail!("monitor:{} is already registered", name);
        }

        let name = name.to_string();
        tokio::spawn(async move {
            let monitors = match hyprctl_monitors().await {
                Ok(m) => m,
                Err(err) => {
                    tracing::error!(%err, "failed to fetch monitor info");
                    return
                },
            };

            let info = match monitors.iter().find(|m| m.name == name) {
                Some(info) => info,
                None => {
                    tracing::error!("no such window: name={}", name);
                    return
                },
            };

            let monitor = Monitor {
                id: info.id.into(),
                name: info.name.to_string(),
                state: State::new(),
            };

            if let Err(err) = tx.send(Ctrl::MonitorAdded(monitor)).await {
                tracing::error!(%err, "failed to send Ctrl::MonitorAdded");
            }
        });

        Ok(())
    }

    pub(crate) fn monitor_added_with_object(&mut self, monitor: Monitor) -> anyhow::Result<()> {
        if let Some(_) = self.monitors.iter().find(|m| m.name == monitor.name) {
            bail!("monitor:{} is already registered", monitor.name);
        }

        self.monitors.push(monitor);

        self.reset_monitor_workspaces();

        Ok(())
    }

    fn reset_monitor_workspaces(&self) {
        let args = self.monitors.iter().map(|m| {
            format!(r#"dispatch moveworkspacetomonitor {} {}"#, m.id + 1, m.name)
        }).collect();
        hyprctl_batch(args);
    }
}
