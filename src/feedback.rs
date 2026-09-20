//! Window-local business feedback. Progress stays inline; terminal notices are
//! queued once by business transitions, never inferred from a render pass.

use gpui::{Context, EntityId, SharedString, Window};
use gpui_kit::component::{
    alert::Alert,
    notification::{Notification, NotificationDelivery},
    progress::Progress,
    Sizable as _, WindowExt as _,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    Sync,
    Attachment,
    Settings,
    Send,
    Draft,
}

impl NoticeKind {
    fn key(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Attachment => "attachment",
            Self::Settings => "settings",
            Self::Send => "send",
            Self::Draft => "draft",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notice {
    pub kind: NoticeKind,
    pub level: NoticeLevel,
    pub title: SharedString,
    pub message: SharedString,
}

struct BusinessNotification;

impl Notice {
    fn into_notification(self, owner: EntityId) -> Notification {
        let notification = match self.level {
            NoticeLevel::Info => Notification::info(self.message),
            NoticeLevel::Success => Notification::success(self.message),
            NoticeLevel::Warning => Notification::warning(self.message),
            NoticeLevel::Error => Notification::error(self.message),
        };
        notification
            .id1::<BusinessNotification>((self.kind.key(), owner))
            .title(self.title)
            // Never inherit an OS-toast delivery mode from global theme settings.
            .delivery(NotificationDelivery::InApp)
            .autohide(matches!(
                self.level,
                NoticeLevel::Info | NoticeLevel::Success
            ))
    }
}

#[derive(Default)]
pub(crate) struct Feedback {
    pending: Vec<Notice>,
}

impl Feedback {
    pub fn push(
        &mut self,
        kind: NoticeKind,
        level: NoticeLevel,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
    ) {
        // Keep the newest result of each channel while the window is hidden or
        // before its next frame. Different operations do not overwrite each other.
        self.pending.retain(|notice| notice.kind != kind);
        self.pending.push(Notice {
            kind,
            level,
            title: title.into(),
            message: message.into(),
        });
    }

    pub fn flush<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if self.pending.is_empty() {
            return;
        }
        let owner = cx.entity().entity_id();
        let pending = std::mem::take(&mut self.pending);
        // Root's layer builders may run while the owner is rendering. Defer the
        // window mutation, and never enqueue again merely because it redraws.
        cx.defer_in(window, move |_, window, cx| {
            for notice in pending {
                window.push_notification(notice.into_notification(owner), cx);
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[Notice] {
        &self.pending
    }
}

pub(crate) fn alert(
    id: &'static str,
    level: NoticeLevel,
    message: impl Into<SharedString>,
) -> Alert {
    let message = message.into();
    match level {
        NoticeLevel::Info => Alert::info(id, message),
        NoticeLevel::Success => Alert::success(id, message),
        NoticeLevel::Warning => Alert::warning(id, message),
        NoticeLevel::Error => Alert::error(id, message),
    }
    .small()
}

pub(crate) fn progress(id: &'static str, value: Option<f32>, label: &'static str) -> Progress {
    Progress::new(id)
        .loading(value.is_none())
        .value(value.unwrap_or(0.))
        .accessibility_label(label)
        .small()
}

pub(crate) fn bounded_percent(done: u64, total: u64) -> Option<f32> {
    if total == 0 {
        None
    } else {
        Some((done as f64 * 100. / total as f64).clamp(0., 100.) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_total_is_indeterminate_not_complete() {
        assert_eq!(bounded_percent(0, 0), None);
        assert_eq!(bounded_percent(30, 0), None);
        assert_eq!(bounded_percent(25, 100), Some(25.));
        assert_eq!(bounded_percent(150, 100), Some(100.));
        assert_eq!(bounded_percent(u64::MAX, u64::MAX), Some(100.));
    }

    #[test]
    fn pending_results_coalesce_within_a_channel() {
        let mut feedback = Feedback::default();
        feedback.push(NoticeKind::Sync, NoticeLevel::Error, "sync", "failed");
        feedback.push(NoticeKind::Sync, NoticeLevel::Success, "sync", "retried");
        assert_eq!(feedback.pending().len(), 1);
        assert_eq!(feedback.pending()[0].level, NoticeLevel::Success);
        assert_eq!(feedback.pending()[0].message.as_ref(), "retried");
    }

    #[test]
    fn independent_business_channels_keep_their_results() {
        let mut feedback = Feedback::default();
        feedback.push(NoticeKind::Sync, NoticeLevel::Success, "sync", "ok");
        feedback.push(NoticeKind::Attachment, NoticeLevel::Error, "save", "failed");
        assert_eq!(feedback.pending().len(), 2);
    }
}
