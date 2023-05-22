use anyhow::bail;

use crate::{state::{State, Changes as MonitorChanges}, hyprctl::MonitorInfo};

#[derive(Debug)]
struct Monitor {
    id: u8,
    name: String,
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
    pub fn next_monitor(&self) -> u8 {
        let next_index = self.active_monitor_index + 1;
        if next_index < self.monitors.len() {
            (next_index as u8) + 1
        } else {
            1
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

    pub fn new_window_added(&mut self, window: String) -> anyhow::Result<()> {
        self.monitors[self.active_monitor_index].state.new_window_added(window)
    }

    pub fn window_removed(&mut self, window: String) -> anyhow::Result<()> {
        self.monitors[self.active_monitor_index].state.window_removed(window)
    }

    pub fn window_moved(&mut self, window: String, dest_monitor: u8) -> anyhow::Result<()> {
        let dest_monitor_index = (dest_monitor - 1) as usize;
        if dest_monitor_index >= 100 {
            // hide windows
            return Ok(())
        }

        let window_removed = self.monitors.iter_mut().find_map(|m| {
            match m.state.window_removed(window.clone()) {
                Ok(_) => Some(true),
                Err(_) => None,
            }
        });

        if window_removed.is_some() {
            self.monitors[dest_monitor_index].state.new_window_added(window)
        } else {
            bail!("no such window: {}", window)
        }
    }

    pub fn focus_window_changed(&mut self, window: String) -> anyhow::Result<()> {
        self.monitors[self.active_monitor_index].state.focus_window_changed(window)
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
}
