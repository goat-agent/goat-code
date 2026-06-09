use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use goat_protocol::Op;

use super::{App, Overlay, QUIT_ARM_TICKS};
use crate::{
    config::ConfigOutcome,
    keymap,
    picker::{EffortOutcome, PickerOutcome, ThreadOutcome},
};

impl App {
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Vec<Op> {
        tracing::trace!(code = ?key.code, modifiers = ?key.modifiers, "key");
        match &self.overlay {
            Overlay::Model(_) => return self.on_picker_key(key),
            Overlay::Effort(_) => return self.on_effort_picker_key(key),
            Overlay::Thread(_) => return self.on_thread_picker_key(key),
            Overlay::Config(_) => return self.on_config_key(key),
            Overlay::Agents(_) => return self.on_agent_selector_key(key),
            Overlay::Commands(_) => {
                if let Some(result) = self.on_command_menu_key(key) {
                    return result;
                }
            }
            Overlay::None => {}
        }
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                return self.on_ctrl_c();
            }
            self.quit_arm = None;
            match ch {
                'a' => {
                    self.dirty |= self.composer.move_home();
                }
                'e' => {
                    self.dirty |= self.composer.move_end();
                }
                'w' => {
                    self.composer.delete_word_before();
                    self.update_command_menu();
                    self.dirty = true;
                }
                _ => {}
            }
            return Vec::new();
        }
        self.quit_arm = None;
        self.on_normal_key(key)
    }

    pub(crate) fn on_command_menu_key(&mut self, key: KeyEvent) -> Option<Vec<Op>> {
        self.dirty = true;
        match key.code {
            KeyCode::Tab => {
                if let Overlay::Commands(menu) = &self.overlay
                    && let Some(name) = menu.selected_name()
                {
                    let completed = format!("/{name} ");
                    self.composer.clear();
                    self.composer.insert_str(&completed);
                }
                self.overlay = Overlay::None;
                Some(Vec::new())
            }
            KeyCode::Enter => {
                if let Overlay::Commands(menu) = &self.overlay
                    && let Some(name) = menu.selected_name()
                {
                    let completed = format!("/{name}");
                    self.overlay = Overlay::None;
                    self.composer.clear();
                    return Some(self.dispatch_slash_command(&completed));
                }
                None
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Some(Vec::new())
            }
            KeyCode::Up => {
                if let Overlay::Commands(menu) = &mut self.overlay {
                    menu.move_up();
                }
                Some(Vec::new())
            }
            KeyCode::Down => {
                if let Overlay::Commands(menu) = &mut self.overlay {
                    menu.move_down();
                }
                Some(Vec::new())
            }
            _ => None,
        }
    }

    pub(crate) fn on_normal_key(&mut self, key: KeyEvent) -> Vec<Op> {
        match key.code {
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                self.dirty = true;
                Vec::new()
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.composer.newline();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Enter => {
                self.overlay = Overlay::None;
                self.dirty = true;
                self.submit()
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Delete => {
                self.composer.delete_forward();
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Left => {
                let changed = if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_left()
                } else {
                    self.composer.move_left()
                };
                self.dirty |= changed;
                Vec::new()
            }
            KeyCode::Right => {
                let changed = if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_right()
                } else {
                    self.composer.move_right()
                };
                self.dirty |= changed;
                Vec::new()
            }
            KeyCode::Home => {
                self.dirty |= self.composer.move_home();
                Vec::new()
            }
            KeyCode::End => {
                self.dirty |= self.composer.move_end();
                Vec::new()
            }
            KeyCode::Up => {
                if self.composer.on_first_row() {
                    self.composer.history_prev();
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_up();
                }
                Vec::new()
            }
            KeyCode::Down => {
                if self.composer.is_empty() && !self.agent_runs.is_empty() {
                    self.set_agent_cursor(0);
                } else if self.composer.on_last_row() {
                    self.composer.history_next();
                    self.dirty = true;
                } else {
                    self.dirty |= self.composer.move_down();
                }
                Vec::new()
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.composer.clear();
                self.dirty = true;
                Vec::new()
            }
            KeyCode::Char(c) => {
                self.composer.insert_char(c);
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn on_picker_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.backspace();
                }
            }
            KeyCode::Enter => {
                if let Overlay::Model(picker) = &mut self.overlay
                    && let PickerOutcome::Selected(target) = picker.choose()
                {
                    self.overlay = Overlay::None;
                    return vec![Op::SelectModel { target }];
                }
            }
            KeyCode::Char(c) => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.on_char(c);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_effort_picker_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => {
                if let Overlay::Effort(picker) = &mut self.overlay {
                    picker.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Effort(picker) = &mut self.overlay {
                    picker.move_down();
                }
            }
            KeyCode::Enter => {
                if let Overlay::Effort(picker) = &self.overlay
                    && let EffortOutcome::Selected(effort) = picker.choose()
                {
                    self.overlay = Overlay::None;
                    return self.apply_effort(effort);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_thread_picker_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => {
                if let Overlay::Thread(picker) = &mut self.overlay {
                    picker.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Thread(picker) = &mut self.overlay {
                    picker.move_down();
                }
            }
            KeyCode::Enter => {
                if let Overlay::Thread(picker) = &self.overlay
                    && let ThreadOutcome::Selected(thread_id) = picker.choose()
                {
                    self.overlay = Overlay::None;
                    return vec![Op::Resume { thread_id }];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_config_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.cancel_stage();
                    if matches!(config.stage_kind(), crate::config::StageKind::List) {
                        self.overlay = Overlay::None;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.tab();
                }
            }
            KeyCode::Up => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.move_up();
                }
            }
            KeyCode::Down => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Overlay::Config(config) = &mut self.overlay {
                    if matches!(config.stage_kind(), crate::config::StageKind::List) {
                        let outcome = config.remove_selected();
                        return self.apply_config_outcome(outcome);
                    }
                    config.backspace();
                }
            }
            KeyCode::Delete => {
                let outcome = if let Overlay::Config(config) = &mut self.overlay {
                    config.remove_selected()
                } else {
                    ConfigOutcome::Pending
                };
                return self.apply_config_outcome(outcome);
            }
            KeyCode::Enter => {
                let outcome = if let Overlay::Config(config) = &mut self.overlay {
                    config.enter()
                } else {
                    ConfigOutcome::Pending
                };
                return self.apply_config_outcome(outcome);
            }
            KeyCode::Char(c) => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.on_char(c);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn on_ctrl_c(&mut self) -> Vec<Op> {
        self.dirty = true;
        if let Some(id) = self.active {
            return vec![Op::Interrupt { id }];
        }
        if self.quit_arm.is_some() {
            self.should_quit = true;
        } else {
            self.quit_arm = Some(QUIT_ARM_TICKS);
        }
        Vec::new()
    }

    pub(crate) fn on_agent_selector_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        match key.code {
            KeyCode::Esc => self.close_agent_selector(),
            KeyCode::Up => match self.agent_selector() {
                Some(0) | None => self.close_agent_selector(),
                Some(cursor) => self.set_agent_cursor(cursor - 1),
            },
            KeyCode::Down => {
                if let Some(cursor) = self.agent_selector()
                    && cursor + 1 < self.agent_runs.len()
                {
                    self.set_agent_cursor(cursor + 1);
                }
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                self.dirty = true;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                self.dirty = true;
            }
            _ => {}
        }
        Vec::new()
    }
}
