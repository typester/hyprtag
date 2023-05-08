use std::{collections::HashSet, hash::Hash};

use anyhow::bail;

#[derive(Debug)]
pub struct State {
    tags: Vec<Tag>,
    visible_tags: u32,
    prev_tags: u32,
    active_tag_index: usize,
    active_window: Option<String>,
}

#[derive(Debug)]
pub struct Changes {
    pub window_added: Vec<WindowInfo>,
    pub window_removed: Vec<WindowInfo>,
    pub focus: Option<String>,
}

#[derive(Debug, Clone, Eq)]
pub struct WindowInfo {
    pub addr: String,
    pub tag: u8,
}

impl PartialEq for WindowInfo {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr
    }
}

impl Hash for WindowInfo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.addr.hash(state);
    }
}

impl State {
    pub fn new() -> Self {
        State {
            tags: (1..=32).map(|n| Tag::new(n)).collect(),
            visible_tags: 1,
            prev_tags: 1,
            active_tag_index: 0,
            active_window: None,
        }
    }

    pub fn visible_tags(&self) -> u32 {
        self.visible_tags
    }

    pub fn set_visible_tags(&mut self, tags: u32) -> anyhow::Result<Changes> {
        if tags == 0 {
            bail!("at least one tag need to be visible");
        }

        let w1 = self.visible_windows();

        self.prev_tags = self.visible_tags;

        let mut first_window = None;
        let mut first_tag_index = None;
        for n in 0..32 {
            if tags & 1<<n != 0 {
                self.visible_tags |= 1<<n;
                if first_window.is_none() && self.tags[n].window_addrs.len() > 0 {
                    first_window = Some(self.tags[n].window_addrs[0].clone());
                }
                if first_tag_index.is_none() {
                    first_tag_index = Some(n);
                }
            } else {
                self.visible_tags &= !(1<<n);
            }
        }

        let w2 = self.visible_windows();

        let (window_added, window_removed) = window_diff(w1, w2);

        let active_tag_index = if let Some(active_window) = &self.active_window {
            self.find_window_tag_index(&active_window)
        } else {
            None
        };

        let focus = if active_tag_index.is_some() && tags & 1<<active_tag_index.unwrap() != 0 {
            self.active_tag_index = active_tag_index.unwrap();
            self.active_window.clone()
        } else {
            self.active_tag_index = first_tag_index.unwrap();
            self.active_window = None;
            first_window
        };

        Ok(Changes {
            window_added,
            window_removed,
            focus,
        })
    }

    pub fn restore_prev_tags(&mut self) -> anyhow::Result<Changes> {
        self.set_visible_tags(self.prev_tags)
    }

    pub fn toggle_tag(&mut self, tag: u8) -> anyhow::Result<Changes> {
        let tag_index = tag - 1;

        let tags = if self.visible_tags & 1<<tag_index != 0 {
            self.visible_tags & !(1<<tag_index)
        } else {
            self.visible_tags | 1<<tag_index
        };

        self.set_visible_tags(tags)
    }

    pub fn new_window_added(&mut self, window: String) -> anyhow::Result<()> {
        if let Some(_) = self.find_window_tag_index(&window) {
            bail!("the window:{} is already in our state", window);
        }

        if let Some(tag) = self.tags.get_mut(self.active_tag_index) {
            tag.window_addrs.push(window);
        }

        Ok(())
    }

    pub fn focus_window_changed(&mut self, window: String) -> anyhow::Result<()> {
        let tag_index = self.find_window_tag_index(&window);

        if tag_index.is_none() {
            self.new_window_added(window.clone())?;
        }

        self.active_window = Some(window);

        Ok(())
    }

    pub fn window_removed(&mut self, window: String) -> anyhow::Result<()> {
        let (tag_index, window_index) = match self.find_window_indexes(&window) {
            Some(indexes) => indexes,
            None => bail!("no such window in our states"),
        };

        if let Some(tag) = self.tags.get_mut(tag_index) {
            tag.window_addrs.remove(window_index);
        }

        Ok(())
    }

    pub fn move_window(&mut self, dest_tag: u8, window: Option<String>) -> anyhow::Result<Changes> {
        let dest_tag_index = (dest_tag - 1) as usize;
        let window = match window.or(self.active_window.clone()) {
            Some(w) => w,
            None => bail!("couldn't find active window"),
        };

        let (tag_index, window_index) = match self.find_window_indexes(&window) {
            Some(indexes) => indexes,
            None => bail!("no such window in our states"),
        };

        if dest_tag_index == tag_index {
            bail!("the window is already in dest tag")
        }

        let w1 = self.visible_windows();

        let tag = match self.tags.get_mut(dest_tag_index) {
            Some(tag) => tag,
            None => bail!(""),
        };
        tag.window_addrs.push(window);

        if let Some(tag) = self.tags.get_mut(tag_index) {
            tag.window_addrs.remove(window_index);
        }

        let w2 = self.visible_windows();

        let (window_added, window_removed) = window_diff(w1, w2);

        Ok(Changes {
            window_added,
            window_removed,
            focus: None,
        })
    }

    pub fn visible_windows(&self) -> Vec<WindowInfo> {
        let mut windows = vec![];
        for n in 0..32 {
            if self.visible_tags & 1<<n != 0 {
                let tag = self.tags.get(n).unwrap();
                windows.extend(tag.window_addrs.iter().map(|w| WindowInfo { addr: w.clone(), tag: tag.id }).collect::<Vec<WindowInfo>>());
            }
        }
        windows
    }

    pub fn find_window_indexes(&self, addr: &str) -> Option<(usize, usize)> {
        self.tags.iter().enumerate().find_map(|(tag_index, tag)| {
            tag.window_addrs.iter().enumerate().find_map(|(window_index, w)| {
                if *w == addr {
                    Some((tag_index, window_index))
                } else {
                    None
                }
            })
        })
    }

    pub fn find_window_tag_index(&self, addr: &str) -> Option<usize> {
        self.tags.iter().enumerate().find_map(|(tag_index, tag)| {
            match tag.window_addrs.iter().find(|w| *w == addr) {
                Some(_) => Some(tag_index),
                None => None,
            }
        })
    }
}

#[derive(Debug)]
pub struct Tag {
    id: u8,
    window_addrs: Vec<String>,
}

impl Tag {
    fn new(id: u8) -> Self {
        Self {
            id,
            window_addrs: vec![],
        }
    }
}

fn window_diff(a: Vec<WindowInfo>, b: Vec<WindowInfo>) -> (Vec<WindowInfo>, Vec<WindowInfo>) {
    let a: HashSet<_> = a.iter().cloned().collect();
    let b: HashSet<_> = b.iter().cloned().collect();

    let added = b.difference(&a).cloned().collect();
    let deleted = a.difference(&b).cloned().collect();

    (added, deleted)
}

#[cfg(test)]
mod tests {
    use super::State;

    fn sorted(v: Vec<String>) -> Vec<String> {
        let mut v = v.clone();
        v.sort();
        v
    }

    #[test]
    fn simple_test() {
        let mut state = State::new();

        state.new_window_added("terminal".into()).unwrap();
        state.new_window_added("firefox".into()).unwrap();
        assert_eq!(state.visible_windows().iter().map(|w| w.addr.clone()).collect::<Vec<String>>(), vec!["terminal", "firefox"]);

        let changes = state.set_visible_tags(1<<1).unwrap();
        assert_eq!(state.visible_windows().len(), 0);
        assert_eq!(changes.window_added.len(), 0);
        assert_eq!(sorted(changes.window_removed.iter().map(|w| w.addr.clone()).collect()), sorted(vec!["terminal".to_string(), "firefox".to_string()]));

        state.set_visible_tags(1<<0).unwrap();
        assert_eq!(state.visible_windows().iter().map(|w| w.addr.clone()).collect::<Vec<String>>(), vec!["terminal", "firefox"]);

        state.move_window(2, Some("firefox".into())).unwrap();
        assert_eq!(state.visible_windows().iter().map(|w| w.addr.clone()).collect::<Vec<String>>(), vec!["terminal"]);

        state.set_visible_tags(1<<1).unwrap();
        assert_eq!(state.visible_windows().iter().map(|w| w.addr.clone()).collect::<Vec<String>>(), vec!["firefox"]);

        state.set_visible_tags(1<<0 | 1<<1).unwrap();
        assert_eq!(state.visible_windows().iter().map(|w| w.addr.clone()).collect::<Vec<String>>(), vec!["terminal", "firefox"]);
    }

    #[test]
    fn toggle_tag() {
        let mut state = State::new();

        state.new_window_added("terminal".into()).unwrap();
        state.new_window_added("firefox".into()).unwrap();
        state.new_window_added("emacs".into()).unwrap();

        state.move_window(2, Some("firefox".into())).unwrap();
        state.move_window(3, Some("emacs".into())).unwrap();

        assert_eq!(state.visible_windows().len(), 1);
        assert_eq!(state.visible_tags(), 0b01);

        state.toggle_tag(2).unwrap();
        assert_eq!(state.visible_windows().len(), 2);
        assert_eq!(state.visible_tags(), 0b11);

        state.toggle_tag(3).unwrap();
        assert_eq!(state.visible_windows().len(), 3);
        assert_eq!(state.visible_tags(), 0b111);

        state.toggle_tag(2).unwrap();
        assert_eq!(state.visible_windows().len(), 2);
        assert_eq!(state.visible_tags(), 0b101);
    }

    #[test]
    fn new_window_on_empty_tag() {
        let mut state = State::new();

        state.new_window_added("terminal".into()).unwrap();

        assert_eq!(state.visible_windows().len(), 1);
        state.set_visible_tags(0b10).unwrap();
        assert_eq!(state.visible_windows().len(), 0);
        assert_eq!(state.active_tag_index, 1);

        state.new_window_added("firefox".into()).unwrap();
        assert_eq!(state.visible_windows().len(), 1);

        state.set_visible_tags(0b1).unwrap();
        assert_eq!(state.visible_windows().len(), 1);
    }

    #[test]
    fn active_tag_index() {
        let mut state = State::new();

        state.focus_window_changed("terminal".into()).unwrap();
        assert_eq!(state.visible_windows().len(), 1);
        assert!(state.active_window.is_some());
        assert_eq!(state.active_tag_index, 0);

        state.set_visible_tags(1<<1).unwrap();
        assert_eq!(state.visible_windows().len(), 0);
        assert_eq!(state.active_tag_index, 1);
        assert!(state.active_window.is_none());
    }
}
