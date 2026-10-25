use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeInputEvidence {
    pub epoch: u64,
    pub potential_submission_epoch: Option<u64>,
    pub uncertain_attachment_epoch: Option<u64>,
    pub known: bool,
}

impl Default for NativeInputEvidence {
    fn default() -> Self {
        Self {
            epoch: 0,
            potential_submission_epoch: None,
            uncertain_attachment_epoch: None,
            known: true,
        }
    }
}

impl NativeInputEvidence {
    pub fn restored_unknown() -> Self {
        Self {
            known: false,
            ..Default::default()
        }
    }

    pub fn text_edit(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    pub fn raw_input(&mut self, bytes: &[u8], bracketed_paste: bool) {
        if bytes.is_empty() {
            return;
        }
        self.text_edit();
        if bracketed_paste {
            if let Some(inner) = bytes
                .strip_prefix(b"\x1b[200~")
                .and_then(|b| b.strip_suffix(b"\x1b[201~"))
            {
                if !inner.contains(&0x1b) {
                    return;
                }
            }
        }
        if bytes
            .iter()
            .any(|&b| b == b'\r' || b == b'\n' || b == 0x1b || b == b'\t')
        {
            self.potential_submission_epoch = Some(self.epoch);
            self.uncertain_attachment_epoch = Some(self.epoch);
        }
    }

    pub fn queue_proven_unchanged_since(&self, baseline: &Self) -> bool {
        if !self.known || !baseline.known {
            return false;
        }
        self.potential_submission_epoch == baseline.potential_submission_epoch
            && self.uncertain_attachment_epoch == baseline.uncertain_attachment_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_text_and_clearing_a_draft_need_native_editor_proof_but_do_not_invent_queued_work() {
        let baseline = NativeInputEvidence::default();
        let mut evidence = baseline;
        evidence.raw_input(b"plain text typing", false);
        assert_eq!(evidence.epoch, 1);
        assert_eq!(evidence.potential_submission_epoch, None);
        assert_eq!(evidence.uncertain_attachment_epoch, None);
        assert!(evidence.queue_proven_unchanged_since(&baseline));

        evidence.text_edit();
        assert_eq!(evidence.epoch, 2);
        assert!(evidence.queue_proven_unchanged_since(&baseline));
    }

    #[test]
    fn negotiated_bracketed_paste_is_editing_but_embedded_escape_is_not_assumed_safe() {
        let baseline = NativeInputEvidence::default();
        let mut safe_paste = baseline;
        safe_paste.raw_input(b"\x1b[200~safe pasted text\x1b[201~", true);
        assert_eq!(safe_paste.potential_submission_epoch, None);
        assert!(safe_paste.queue_proven_unchanged_since(&baseline));

        let mut unsafe_paste = baseline;
        unsafe_paste.raw_input(b"\x1b[200~unsafe\x1bpasted text\x1b[201~", true);
        assert_eq!(unsafe_paste.potential_submission_epoch, Some(1));
        assert!(!unsafe_paste.queue_proven_unchanged_since(&baseline));
    }

    #[test]
    fn raw_enter_tab_mouse_or_attachment_controls_invalidate_native_queue_evidence() {
        let baseline = NativeInputEvidence::default();

        for control in [b"\r".as_slice(), b"\n", b"\t", b"\x1b", b"\x1b[<35;48;26M"] {
            let mut evidence = baseline;
            evidence.raw_input(control, false);
            assert_eq!(evidence.potential_submission_epoch, Some(1));
            assert_eq!(evidence.uncertain_attachment_epoch, Some(1));
            assert!(!evidence.queue_proven_unchanged_since(&baseline));
        }

        let unknown = NativeInputEvidence::restored_unknown();
        assert!(!unknown.queue_proven_unchanged_since(&baseline));
        assert!(!baseline.queue_proven_unchanged_since(&unknown));
    }
}
