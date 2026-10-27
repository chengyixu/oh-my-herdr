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
        // Kitty's Ctrl+C encoding is native cancellation, and focus reports do
        // not edit or submit the composer. Neither implies a queued prompt.
        if matches!(
            bytes,
            b"\x03"
                | b"\x1b[99;5u"
                | b"\x1b[99;5:1u"
                | b"\x1b[99;5:2u"
                | b"\x1b[99;5:3u"
                | b"\x1b[I"
                | b"\x1b[O"
        ) {
            return;
        }
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
        if bytes.iter().any(|&b| b == b'\r' || b == b'\n') {
            self.potential_submission_epoch = Some(self.epoch);
        }
        if bytes.iter().any(|&b| b == 0x1b || b == b'\t' || b == 0x16) {
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
        assert_eq!(unsafe_paste.uncertain_attachment_epoch, Some(1));
        assert!(!unsafe_paste.queue_proven_unchanged_since(&baseline));
    }

    #[tokio::test]
    async fn a_new_native_owner_does_not_inherit_the_previous_process_input_uncertainty() {
        let (runtime, _receiver) = crate::pane::PaneRuntime::test_with_channel(80, 24);
        runtime.set_full_lifecycle_authority_active(true);
        runtime.set_self_reported_agent_active(true);
        runtime
            .native_input
            .set(NativeInputEvidence::restored_unknown());
        let previous = runtime.native_input_evidence();
        assert!(!previous.known);
        runtime.begin_native_process_scope();
        assert!(!runtime
            .full_lifecycle_authority_active
            .load(std::sync::atomic::Ordering::Acquire));
        assert!(!runtime
            .self_reported_agent_active
            .load(std::sync::atomic::Ordering::Acquire));
        let baseline = runtime.native_input_evidence();
        assert!(baseline.known && baseline.epoch > previous.epoch);
        assert!(baseline.uncertain_attachment_epoch.is_none());
        runtime
            .try_send_bytes(bytes::Bytes::from_static(b"\x1b"))
            .unwrap();
        assert!(!runtime
            .native_input_evidence()
            .queue_proven_unchanged_since(&baseline));
    }

    #[test]
    fn native_cancel_and_focus_reports_do_not_invent_queued_work() {
        for bytes in [
            b"\x03".as_slice(),
            b"\x1b[99;5u",
            b"\x1b[99;5:1u",
            b"\x1b[99;5:2u",
            b"\x1b[99;5:3u",
            b"\x1b[I",
            b"\x1b[O",
        ] {
            let baseline = NativeInputEvidence::default();
            let mut evidence = baseline;
            evidence.raw_input(bytes, false);
            assert!(
                evidence.queue_proven_unchanged_since(&baseline),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn raw_enter_tab_mouse_or_attachment_controls_invalidate_native_queue_evidence() {
        let baseline = NativeInputEvidence::default();

        for control in [b"\r".as_slice(), b"\n", b"\t", b"\x1b", b"\x1b[<35;48;26M"] {
            let mut evidence = baseline;
            evidence.raw_input(control, false);
            if control == b"\r" || control == b"\n" {
                assert_eq!(evidence.potential_submission_epoch, Some(1));
            } else {
                assert_eq!(evidence.uncertain_attachment_epoch, Some(1));
            }
            assert!(!evidence.queue_proven_unchanged_since(&baseline));
        }

        let unknown = NativeInputEvidence::restored_unknown();
        assert!(!unknown.queue_proven_unchanged_since(&baseline));
        assert!(!baseline.queue_proven_unchanged_since(&unknown));
    }
}
