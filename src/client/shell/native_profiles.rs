use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[derive(Debug)]
pub(super) enum ProfileAction {
    Cancel,
    SaveAgentProfile,
    CreateAgentProfileMarkdown { role: String, name: String },
    DeleteAgentProfileMarkdown { role: String, name: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileMarkdown {
    pub name: String,
    pub path: String,
    pub content: String,
    /// Byte offset at a UTF-8 boundary in `content`.
    pub cursor: usize,
    /// First rendered content row shown in the editor viewport.
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileForm {
    /// Existing roles are immutable because they are the durable profile and
    /// roster identity. `None` represents a new profile.
    pub existing_role: Option<String>,
    pub role: String,
    pub harness: String,
    pub harness_choices: Vec<String>,
    pub harness_settings: Vec<crate::api::schema::HarnessIntegrationInfo>,
    pub native_cwd: String,
    /// Empty means the harness default is used.
    pub model: String,
    /// Empty means the harness default is used.
    pub effort: String,
    /// Empty means no environment-variable reference is configured.
    pub apikey_ref: String,
    /// Empty means no tool allowlist is configured. Non-empty values are JSON.
    pub allowlist: String,
    /// Other profile-owned Markdown files injected alongside `AGENTS.md`.
    pub additional_markdown: Vec<AgentProfileMarkdown>,
    /// Existing Markdown attachments outside this profile's document cabinet.
    pub linked_markdown: Vec<String>,
    pub instructions: String,
    /// Byte offset at a UTF-8 boundary in `instructions`.
    pub instructions_cursor: usize,
    /// First rendered instruction row shown in the editor viewport.
    pub instructions_scroll: usize,
    /// `None` selects `AGENTS.md`; an index selects an additional document.
    pub selected_markdown: Option<usize>,
    /// A filename being entered for a new profile-owned Markdown document.
    pub pending_markdown_name: Option<String>,
    pub selected_field: usize,
}

impl AgentProfileForm {
    pub fn is_new(&self) -> bool {
        self.existing_role.is_none()
    }

    pub fn field_count(&self) -> usize {
        if self.is_new() {
            4
        } else {
            8
        }
    }

    pub fn capabilities(&self) -> crate::agent_registry::NativeHarnessCapabilities {
        self.harness_settings
            .iter()
            .find(|h| h.id == self.harness)
            .map(|h| h.capabilities)
            .unwrap_or_else(|| crate::agent_registry::native_harness_capabilities(&self.harness))
    }

    pub fn profile_instructions_supported(&self) -> bool {
        self.harness_settings
            .iter()
            .find(|h| h.id == self.harness)
            .map(|h| h.profile_instructions)
            .unwrap_or_else(|| {
                crate::harness::find(&self.harness)
                    .is_some_and(|h| h.instructions != crate::harness::Instructions::Native)
            })
    }

    pub fn field_enabled(&self, field: usize) -> bool {
        if self.is_new() {
            return field < self.field_count();
        }
        let capabilities = self.capabilities();
        match field {
            2 => capabilities.model,
            3 => capabilities.effort,
            4 => capabilities.api_key_ref,
            5 => capabilities.tool_allowlist,
            _ => field < self.field_count(),
        }
    }

    pub fn visible_fields(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.field_count()).filter(|field| self.field_enabled(*field))
    }

    pub fn documents_field(&self) -> Option<usize> {
        (!self.is_new()).then_some(6)
    }

    pub fn instructions_field(&self) -> usize {
        if self.is_new() {
            3
        } else {
            7
        }
    }

    pub fn instructions_selected(&self) -> bool {
        self.selected_field == self.instructions_field()
    }

    pub fn documents_selected(&self) -> bool {
        self.documents_field() == Some(self.selected_field)
    }

    pub fn selected_markdown_index(&self) -> Option<usize> {
        self.selected_markdown
            .filter(|index| *index < self.additional_markdown.len())
    }

    pub fn active_document_name(&self) -> &str {
        self.selected_markdown_index()
            .map(|index| self.additional_markdown[index].name.as_str())
            .unwrap_or("AGENTS.md")
    }

    pub fn active_document_content(&self) -> &str {
        self.selected_markdown_index()
            .map(|index| self.additional_markdown[index].content.as_str())
            .unwrap_or(&self.instructions)
    }

    pub fn active_document_scroll(&self) -> usize {
        self.selected_markdown_index()
            .map(|index| self.additional_markdown[index].scroll)
            .unwrap_or(self.instructions_scroll)
    }

    pub fn active_document_cursor(&self) -> usize {
        self.selected_markdown_index()
            .map(|index| self.additional_markdown[index].cursor)
            .unwrap_or(self.instructions_cursor)
    }

    pub fn active_document_mut(&mut self) -> (&mut String, &mut usize, &mut usize) {
        let selected = self.selected_markdown_index();
        match selected {
            Some(index) => {
                let document = &mut self.additional_markdown[index];
                (
                    &mut document.content,
                    &mut document.cursor,
                    &mut document.scroll,
                )
            }
            None => (
                &mut self.instructions,
                &mut self.instructions_cursor,
                &mut self.instructions_scroll,
            ),
        }
    }

    pub fn cycle_document(&mut self, forward: bool) {
        let len = self.additional_markdown.len();
        if len == 0 {
            self.selected_markdown = None;
            return;
        }
        self.selected_markdown = match self.selected_markdown_index() {
            None if forward => Some(0),
            None => Some(len - 1),
            Some(0) if !forward => None,
            Some(index) if forward && index + 1 == len => None,
            Some(index) if forward => Some(index + 1),
            Some(index) => Some(index - 1),
        };
    }
}
pub(super) fn edit_form(form: &mut AgentProfileForm, key: KeyEvent) -> Option<ProfileAction> {
    if let Some(name) = form.pending_markdown_name.as_mut() {
        match key.code {
            KeyCode::Esc => form.pending_markdown_name = None,
            KeyCode::Backspace => {
                name.pop();
            }
            KeyCode::Enter => {
                if let Some(role) = form.existing_role.clone() {
                    if !name.trim().is_empty() {
                        return Some(ProfileAction::CreateAgentProfileMarkdown {
                            role,
                            name: name.trim().to_string(),
                        });
                    }
                }
            }
            KeyCode::Char(ch) if !ch.is_control() => name.push(ch),
            _ => {}
        }
        return None;
    }
    match key.code {
        KeyCode::Esc => {
            return Some(ProfileAction::Cancel);
        }
        KeyCode::Up if form.instructions_selected() => {
            move_agent_profile_instruction_cursor_vertical(form, false)
        }
        KeyCode::Down if form.instructions_selected() => {
            move_agent_profile_instruction_cursor_vertical(form, true)
        }
        KeyCode::Up | KeyCode::BackTab => {
            let field = form
                .visible_fields()
                .filter(|field| *field < form.selected_field)
                .last();
            if let Some(field) = field {
                form.selected_field = field;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            let field = form
                .visible_fields()
                .find(|field| *field > form.selected_field);
            if let Some(field) = field {
                form.selected_field = field;
            }
        }
        KeyCode::Left => move_agent_profile_instruction_cursor(form, false),
        KeyCode::Right => move_agent_profile_instruction_cursor(form, true),
        KeyCode::Char('a') if form.documents_selected() => {
            form.pending_markdown_name = Some(String::new());
        }
        KeyCode::Char('d') if form.documents_selected() => {
            if let Some(index) = form.selected_markdown_index() {
                if let Some(role) = form.existing_role.clone() {
                    return Some(ProfileAction::DeleteAgentProfileMarkdown {
                        role,
                        name: form.additional_markdown[index].name.clone(),
                    });
                }
            }
        }
        KeyCode::Backspace => {
            if form.instructions_selected() {
                delete_agent_profile_instruction_char(form, true);
            } else if let Some(value) = agent_profile_form_field_mut(form) {
                value.pop();
            }
        }
        KeyCode::Delete if form.instructions_selected() => {
            delete_agent_profile_instruction_char(form, false);
        }
        KeyCode::PageUp if form.instructions_selected() => {
            let (_, _, scroll) = form.active_document_mut();
            *scroll = scroll.saturating_sub(8);
        }
        KeyCode::PageDown if form.instructions_selected() => {
            let (_, _, scroll) = form.active_document_mut();
            *scroll = scroll.saturating_add(8);
        }
        KeyCode::Home if form.instructions_selected() => {
            let (content, cursor, _) = form.active_document_mut();
            *cursor = instruction_line_start(content, *cursor);
        }
        KeyCode::End if form.instructions_selected() => {
            let (content, cursor, _) = form.active_document_mut();
            *cursor = instruction_line_end(content, *cursor);
        }
        KeyCode::Enter
            if form.instructions_selected() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            insert_agent_profile_instructions(form, "\n");
        }
        KeyCode::Enter => return Some(ProfileAction::SaveAgentProfile),
        KeyCode::Char(ch) if !ch.is_control() => {
            if form.instructions_selected() {
                insert_agent_profile_instructions(form, &ch.to_string());
            } else if let Some(value) = agent_profile_form_field_mut(form) {
                value.push(ch);
            }
        }
        _ => {}
    }
    None
}

fn agent_profile_form_field_mut(form: &mut AgentProfileForm) -> Option<&mut String> {
    if !form.field_enabled(form.selected_field) {
        return None;
    }
    match (form.is_new(), form.selected_field) {
        (true, 0) => Some(&mut form.role),
        (true, 2) | (false, 1) => Some(&mut form.native_cwd),
        (false, 2) => Some(&mut form.model),
        (false, 4) => Some(&mut form.apikey_ref),
        (false, 5) => Some(&mut form.allowlist),
        _ => None,
    }
}

fn move_agent_profile_instruction_cursor(form: &mut AgentProfileForm, forward: bool) {
    if form.instructions_selected() {
        let (content, cursor, _) = form.active_document_mut();
        let current = (*cursor).min(content.len());
        *cursor = if forward {
            next_instruction_boundary(content, current)
        } else {
            previous_instruction_boundary(content, current)
        };
        return;
    }

    if form.documents_selected() {
        form.cycle_document(forward);
        return;
    }

    let harness_field = if form.is_new() { 1 } else { 0 };
    if form.selected_field == harness_field {
        let harnesses = &form.harness_choices;
        if harnesses.is_empty() {
            return;
        }
        let current = harnesses
            .iter()
            .position(|harness| *harness == form.harness);
        let next = match current {
            Some(current) if forward => (current + 1) % harnesses.len(),
            Some(current) => (current + harnesses.len() - 1) % harnesses.len(),
            None if forward => 0,
            None => harnesses.len() - 1,
        };
        form.harness = harnesses[next].clone();
        return;
    }

    if !form.is_new() && form.selected_field == 3 {
        const EFFORTS: [&str; 4] = ["", "low", "medium", "high"];
        let current = EFFORTS
            .iter()
            .position(|effort| *effort == form.effort)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % EFFORTS.len()
        } else {
            (current + EFFORTS.len() - 1) % EFFORTS.len()
        };
        form.effort = EFFORTS[next].to_string();
    }
}

pub(super) fn insert_agent_profile_form_text(form: &mut AgentProfileForm, text: &str) -> bool {
    if let Some(name) = form.pending_markdown_name.as_mut() {
        name.extend(text.chars().filter(|ch| !ch.is_control()));
        return true;
    }
    if form.instructions_selected() {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let text: String = text
            .chars()
            .filter(|ch| *ch == '\n' || !ch.is_control())
            .collect();
        insert_agent_profile_instructions(form, &text);
        return true;
    }
    let Some(value) = agent_profile_form_field_mut(form) else {
        return false;
    };
    value.extend(text.chars().filter(|ch| !ch.is_control()));
    true
}

fn insert_agent_profile_instructions(form: &mut AgentProfileForm, text: &str) {
    let (content, cursor, _) = form.active_document_mut();
    let current = (*cursor).min(content.len());
    content.insert_str(current, text);
    *cursor = current + text.len();
}

fn move_agent_profile_instruction_cursor_vertical(form: &mut AgentProfileForm, down: bool) {
    let (content, cursor, _) = form.active_document_mut();
    let current = (*cursor).min(content.len());
    let current_start = instruction_line_start(content, current);
    let current_end = instruction_line_end(content, current);
    let desired_column = instruction_display_width(&content[current_start..current]);

    let (target_start, target_end) = if down {
        if current_end == content.len() {
            return;
        }
        let start = current_end + 1;
        (start, instruction_line_end(content, start))
    } else {
        if current_start == 0 {
            return;
        }
        let end = current_start - 1;
        (instruction_line_start(content, end), end)
    };
    *cursor =
        instruction_offset_at_display_width(content, target_start, target_end, desired_column);
}

fn instruction_display_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn instruction_offset_at_display_width(
    value: &str,
    start: usize,
    end: usize,
    desired_width: usize,
) -> usize {
    let mut offset = start;
    let mut width = 0;
    for (index, ch) in value[start..end].char_indices() {
        let next_width = width + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if next_width > desired_width {
            break;
        }
        width = next_width;
        offset = start + index + ch.len_utf8();
    }
    offset
}

fn previous_instruction_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor.min(value.len())]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_instruction_boundary(value: &str, cursor: usize) -> usize {
    let cursor = cursor.min(value.len());
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .unwrap_or(value.len())
}

fn instruction_line_start(value: &str, cursor: usize) -> usize {
    value[..cursor.min(value.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn instruction_line_end(value: &str, cursor: usize) -> usize {
    let cursor = cursor.min(value.len());
    value[cursor..]
        .find('\n')
        .map(|index| cursor + index)
        .unwrap_or(value.len())
}

fn delete_agent_profile_instruction_char(form: &mut AgentProfileForm, backwards: bool) {
    let (content, cursor, _) = form.active_document_mut();
    let current = (*cursor).min(content.len());
    let (start, end) = if backwards {
        (previous_instruction_boundary(content, current), current)
    } else {
        (current, next_instruction_boundary(content, current))
    };
    if start == end {
        return;
    }
    content.replace_range(start..end, "");
    *cursor = start;
}

use ratatui::{
    buffer::Buffer,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget, Wrap},
};
fn agent_profile_document_text<'a>(content: &'a str, cursor: usize, show_cursor: bool) -> Text<'a> {
    let cursor = normalized_document_cursor(content, cursor);
    let cursor_style = Style::default().add_modifier(Modifier::REVERSED);
    let mut line_start = 0;
    let mut lines = Vec::new();

    for line in content.split('\n') {
        let line_end = line_start + line.len();
        let spans = if show_cursor && (line_start..=line_end).contains(&cursor) {
            let cursor_in_line = cursor - line_start;
            let (before, after) = line.split_at(cursor_in_line);
            let mut spans = vec![Span::raw(before)];
            if let Some(ch) = after.chars().next() {
                let ch_len = ch.len_utf8();
                spans.push(Span::styled(&after[..ch_len], cursor_style));
                spans.push(Span::raw(&after[ch_len..]));
            } else {
                spans.push(Span::styled(" ", cursor_style));
            }
            spans
        } else {
            vec![Span::raw(line)]
        };
        lines.push(Line::from(spans));
        line_start = line_end.saturating_add(1);
    }

    Text::from(lines)
}

fn normalized_document_cursor(content: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(content.len());
    while cursor > 0 && !content.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn document_caret_position(document: &Text<'_>, width: u16) -> Option<(u16, usize, usize)> {
    if width == 0 {
        return None;
    }
    let paragraph = Paragraph::new(document.clone()).wrap(Wrap { trim: false });
    let line_count = paragraph.line_count(width).max(1);
    let height = line_count.min(u16::MAX as usize) as u16;
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    paragraph.render(area, &mut buffer);

    buffer
        .content
        .iter()
        .position(|cell| cell.style().add_modifier.contains(Modifier::REVERSED))
        .map(|index| {
            (
                (index % width as usize) as u16,
                index / width as usize,
                line_count,
            )
        })
}

pub(super) fn render_document(
    buffer: &mut Buffer,
    area: Rect,
    form: &AgentProfileForm,
    style: Style,
) {
    let document = agent_profile_document_text(
        form.active_document_content(),
        form.active_document_cursor(),
        form.instructions_selected(),
    );
    let mut scroll = form.active_document_scroll();
    if let Some((_, caret_y, lines)) = document_caret_position(&document, area.width) {
        scroll = scroll.min(lines.saturating_sub(area.height as usize));
        if caret_y < scroll {
            scroll = caret_y;
        }
        if caret_y >= scroll + area.height as usize {
            scroll = caret_y.saturating_sub(area.height.saturating_sub(1) as usize);
        }
    }
    Paragraph::new(document)
        .style(style)
        .wrap(Wrap { trim: false })
        .scroll((scroll.min(u16::MAX as usize) as u16, 0))
        .render(area, buffer);
}
