//! Closed-session history: KelpServer's records (`Entry`, `History`,
//! `collect`), plus the restore choices KelpMux offers for them.
//! `RestoreChoice` and `restore_choices` were copied from KelpServer's
//! `native::history` (kelpserver afc5197, Apache-2.0, same authors), which
//! removed them in 0.3.0-beta.2; `restore_choices` is a trait method here
//! because `Entry` is KelpServer's type.
pub use kelpserver::native::history::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreChoice {
    Resume,
    Picker,
    Restart,
    Folder,
    Cancel,
}
impl RestoreChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume conversation",
            Self::Picker => "Open conversation picker",
            Self::Restart => "Restart program",
            Self::Folder => "Open folder",
            Self::Cancel => "Cancel",
        }
    }
}
/// The choices the Closed dialog offers for one closed session.
pub trait RestoreChoices {
    fn restore_choices(&self) -> Vec<RestoreChoice>;
}
impl RestoreChoices for Entry {
    fn restore_choices(&self) -> Vec<RestoreChoice> {
        use RestoreChoice::*;
        if self.kind.is_some() {
            vec![
                if self.session_id.as_deref().is_some_and(crate::session_id) {
                    Resume
                } else {
                    Picker
                },
                Cancel,
            ]
        } else if self.previous_command().is_some() {
            vec![Restart, Folder, Cancel]
        } else {
            vec![Folder, Cancel]
        }
    }
}
