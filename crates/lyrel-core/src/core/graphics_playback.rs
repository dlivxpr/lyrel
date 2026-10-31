//! Living-daemon playback. A single pane worker sleeps until the earliest
//! visible deadline; visibility withdrawal freezes duration, not wall-clock time.
use super::*;
use std::time::{Duration, Instant};

pub(super) struct Animation {
    pub(super) mode: u8,
    loops: u32,
    completed: u32,
    remaining: Duration,
    deadline: Option<Instant>,
    visible: bool,
    waiting: bool,
}

impl GraphicsState {
    fn playback_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.controlled_now { return now; }
        Instant::now()
    }

    fn wake_playback(&self) {
        if let Some(wake) = &self.wake { let _ = wake.try_send(()); }
    }

    pub(super) fn frame_key(&self, root: u64, number: u32) -> Option<u64> {
        if number == 1 { return self.resources.get(&root).map(|r| r.content_key); }
        self.resources.values().find_map(|resource| {
            (resource.root == Some(root) && resource.frame_number == number).then_some(resource.content_key)
        })
    }

    fn frame_resource(&self, root: u64, content: u64) -> Option<&Resource> {
        self.resources.iter().find_map(|(&key, resource)| {
            ((key == root || resource.root == Some(root)) && resource.content_key == content).then_some(resource)
        })
    }

    pub(super) fn select_content(&mut self, root: u64, frame: u64, effects: &mut Vec<GraphicsEffect>) {
        let resource = self.resources.get_mut(&root).unwrap();
        resource.selected = frame;
        resource.revision = resource.revision.checked_add(1).expect("graphics selection revision exhausted");
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Select { key: root, frame, revision: resource.revision }));
    }

    pub(super) fn playback_snapshot(&self, root: u64) -> Playback {
        let animation = &self.animations[&root];
        let remaining = animation.deadline.map_or(animation.remaining, |deadline| deadline.saturating_duration_since(self.playback_now()));
        Playback { key: root, state: animation.mode, remaining_ms: remaining.as_millis().min(u128::from(u64::MAX)) as u64,
            loops: animation.loops, completed: animation.completed, paused: !animation.visible, waiting: animation.waiting }
    }

    pub(super) fn drain_playback_metadata(&mut self) -> Vec<GraphicsEffect> {
        std::mem::take(&mut self.playback_dirty).into_iter().filter_map(|key| {
            self.animations.contains_key(&key).then(|| GraphicsEffect::Update(GraphicsUpdate::Playback(self.playback_snapshot(key))))
        }).collect()
    }

    pub(super) fn sync_playback_visibility(&mut self) {
        let now = self.playback_now();
        let before = self.animations.len();
        self.animations.retain(|key, _| self.resources.get(key).is_some_and(|r| r.resident.lock().unwrap().packet.is_some()));
        let mut changed = before != self.animations.len();
        for (&key, animation) in &mut self.animations {
            let visible = self.resources[&key].visible;
            if animation.visible == visible { continue; }
            if visible {
                if animation.mode != 1 && !animation.waiting { animation.deadline = Some(now + animation.remaining); }
            } else if let Some(deadline) = animation.deadline.take() {
                animation.remaining = deadline.saturating_duration_since(now);
            }
            animation.visible = visible;
            self.playback_dirty.insert(key);
            changed = true;
        }
        if changed { self.wake_playback(); }
    }

    pub(crate) fn playback_delay(&self) -> Option<Duration> {
        let now = self.playback_now();
        self.animations.values().filter_map(|animation| animation.deadline).min().map(|deadline| deadline.saturating_duration_since(now))
    }

    pub(super) fn frame_committed(&mut self, root: u64, key: u64, control: &Control, effects: &mut Vec<GraphicsEffect>) {
        let gap_changed = control.action == Action::Frame && control.z != 0;
        if gap_changed { self.resources.get_mut(&key).unwrap().gap_ms = control.z.max(0) as u32; }
        let now = self.playback_now();
        let content = self.resources[&key].content_key;
        let gap = self.resources[&key].gap_ms;
        let selected = self.resources[&root].selected;
        let selected_number = self.frame_resource(root, selected).unwrap().frame_number;
        let has_next = self.resources.values().any(|r| r.root == Some(root) && r.frame_number > selected_number);
        if let Some(animation) = self.animations.get_mut(&root) {
            if animation.waiting && (has_next || (animation.mode == 3 && gap_changed)) {
                animation.waiting = false;
                animation.remaining = Duration::ZERO;
                animation.deadline = (animation.visible && animation.mode != 1).then_some(now);
            } else if !animation.waiting && selected == content && gap_changed {
                animation.remaining = Duration::from_millis(u64::from(gap));
                animation.deadline = (animation.visible && animation.mode != 1).then_some(now + animation.remaining);
            }
            self.playback_dirty.insert(root);
            effects.extend(self.drain_playback_metadata());
            self.wake_playback();
        }
    }

    pub(super) fn frame_deleted(&mut self, root: u64, replaced_current: bool, effects: &mut Vec<GraphicsEffect>) {
        let now = self.playback_now();
        let selected = self.resources[&root].selected;
        let frame = self.frame_resource(root, selected).unwrap();
        let gap = Duration::from_millis(u64::from(frame.gap_ms));
        let has_next = self.resources.values().any(|r| r.root == Some(root) && r.frame_number > frame.frame_number);
        if let Some(animation) = self.animations.get_mut(&root) {
            if animation.waiting {
                if has_next {
                    animation.waiting = false;
                    animation.remaining = Duration::ZERO;
                    animation.deadline = (animation.visible && animation.mode != 1).then_some(now);
                }
            } else if replaced_current {
                animation.remaining = gap;
                animation.deadline = (animation.visible && animation.mode != 1).then_some(now + gap);
            }
            self.playback_dirty.insert(root);
            effects.extend(self.drain_playback_metadata());
            self.wake_playback();
        }
    }

    pub(super) fn control_animation(&mut self, control: &Control, effects: &mut Vec<GraphicsEffect>) {
        let Some(root) = self.lookup(control) else {
            Self::reply(effects, control, control.id, "ENOENT:Image not found"); return;
        };
        let id = self.resources[&root].id;
        if control.width > 3 {
            Self::reply(effects, control, id, "EINVAL:Invalid animation state"); return;
        }
        let selected = self.resources[&root].selected;
        let frame = if control.cols == 0 { Some(selected) } else { self.frame_key(root, control.cols) };
        let gap_target = if control.z == 0 { Some(selected) } else { self.frame_key(root, control.rows.max(1)) };
        let Some((frame, gap_target)) = frame.zip(gap_target).filter(|(frame, target)| {
            [*frame, *target].iter().all(|key| self.frame_resource(root, *key).is_some_and(|r| r.resident.lock().unwrap().packet.is_some()))
        }) else {
            Self::reply(effects, control, id, "ENOENT:Animation frame not found"); return;
        };
        // Validate every reference before changing either timing or selection.
        if control.z != 0 {
            let resource = self.resources.values_mut().find(|resource| resource.content_key == gap_target).unwrap();
            resource.gap_ms = control.z.max(0) as u32;
        }
        let now = self.playback_now();
        let gap = Duration::from_millis(u64::from(self.frame_resource(root, frame).unwrap().gap_ms));
        let visible = self.resources[&root].visible;
        let animation = self.animations.entry(root).or_insert(Animation {
            mode: 1, loops: 0, completed: 0, remaining: gap, deadline: None, visible, waiting: false,
        });
        if control.height != 0 { animation.loops = control.height - 1; }
        if control.cols != 0 || (control.z != 0 && gap_target == frame) {
            animation.remaining = gap;
            animation.waiting = false;
            animation.deadline = None;
        }
        if animation.mode == 3 && animation.waiting && control.z != 0 {
            animation.waiting = false;
            animation.remaining = Duration::ZERO;
            animation.deadline = None;
        }
        if control.width != 0 {
            if control.width == 1 {
                if let Some(deadline) = animation.deadline.take() { animation.remaining = deadline.saturating_duration_since(now); }
                animation.completed = 0;
            }
            if animation.mode != control.width as u8 { animation.waiting = false; }
            animation.mode = control.width as u8;
        }
        if animation.mode != 1 && animation.visible && !animation.waiting && animation.deadline.is_none() {
            animation.deadline = Some(now + animation.remaining);
        }
        if control.cols != 0 { self.select_content(root, frame, effects); }
        self.playback_dirty.insert(root);
        effects.extend(self.drain_playback_metadata());
        Self::reply(effects, control, id, "OK");
        self.wake_playback();
    }

    pub(crate) fn advance_playback(&mut self) -> Vec<GraphicsEffect> {
        // Layout/viewport mutations already reduce the authoritative graph.
        // A timer tick only retires evicted clocks, never rescans the text grid.
        self.sync_playback_visibility();
        let now = self.playback_now();
        let mut effects = Vec::new();
        // Remove each timeline temporarily to keep the immutable frame graph and
        // mutable clock disjoint without copying the retained pixel sequence.
        let mut animations = std::mem::take(&mut self.animations);
        for (&root, animation) in &mut animations {
            let selected = self.resources[&root].selected;
            let mut frame = selected;
            let mut presented = selected;
            let before = (animation.mode, animation.completed, animation.waiting);
            let cycle_ms: u64 = self.resources.iter().filter(|(key, resource)| **key == root || resource.root == Some(root)).map(|(_, r)| u64::from(r.gap_ms)).sum();
            if cycle_ms == 0 && animation.deadline.is_some() {
                if animation.mode == 3 && animation.loops != 0 {
                    animation.completed = animation.loops;
                    animation.mode = 1;
                } else {
                    animation.waiting = true;
                }
                animation.deadline = None;
            } else if animation.mode == 3 && let Some(deadline) = animation.deadline {
                let cycles = now.saturating_duration_since(deadline).as_millis() / u128::from(cycle_ms.max(1));
                let available = if animation.loops == 0 { u32::MAX } else { animation.loops.saturating_sub(animation.completed).saturating_sub(1) };
                let cycles = cycles.min(u128::from(available)) as u32;
                animation.completed = animation.completed.saturating_add(cycles);
                animation.deadline = Some(deadline + Duration::from_millis(cycle_ms * u64::from(cycles)));
            }
            let mut steps = 0;
            while animation.deadline.is_some_and(|deadline| deadline <= now) {
                let current = self.frame_resource(root, frame).unwrap();
                let next = self.resources.iter().filter(|(key, resource)| **key == root || resource.root == Some(root))
                    .filter(|(_, resource)| resource.frame_number > current.frame_number)
                    .min_by_key(|(_, resource)| resource.frame_number).map(|(_, resource)| resource.content_key);
                let next = if let Some(next) = next { next } else {
                    if animation.mode == 2 {
                        animation.waiting = true; animation.remaining = Duration::ZERO; animation.deadline = None; break;
                    }
                    animation.completed = animation.completed.saturating_add(1);
                    if animation.loops != 0 && animation.completed >= animation.loops {
                        animation.mode = 1; animation.deadline = None; animation.remaining = Duration::ZERO; break;
                    }
                    self.resources[&root].content_key
                };
                frame = next;
                let gap = Duration::from_millis(u64::from(self.frame_resource(root, next).unwrap().gap_ms));
                if !gap.is_zero() { presented = next; }
                animation.remaining = gap;
                animation.deadline = animation.deadline.map(|deadline| deadline + gap);
                steps += 1;
                // A sequence containing only gapless frames has no presentable
                // deadline. Park it until an explicit edit/control/append wakes it.
                if steps >= MAX_ANIMATION_FRAMES as usize * 2 {
                    animation.deadline = Some(now + gap);
                    break;
                }
            }
            if presented != selected { self.select_content(root, presented, &mut effects); }
            if steps != 0 || before != (animation.mode, animation.completed, animation.waiting) {
                self.playback_dirty.insert(root);
            }
        }
        self.animations = animations;
        effects.extend(self.drain_playback_metadata());
        effects
    }
}
