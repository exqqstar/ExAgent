use std::collections::VecDeque;

use anyhow::Result;
use tokio::sync::{watch, Mutex};

use crate::runtime::subagent::InterAgentCommunication;
use crate::types::ThreadId;

#[derive(Debug)]
pub struct ThreadInbox {
    thread_id: ThreadId,
    mailbox_tx: watch::Sender<()>,
    queue: Mutex<VecDeque<InterAgentCommunication>>,
}

impl ThreadInbox {
    pub(crate) fn new(thread_id: ThreadId) -> Self {
        let (mailbox_tx, _) = watch::channel(());
        Self {
            thread_id,
            mailbox_tx,
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) async fn enqueue(&self, mail: InterAgentCommunication) -> Result<()> {
        if mail.recipient_thread_id != self.thread_id {
            anyhow::bail!(
                "mail recipient {} does not match thread {}",
                mail.recipient_thread_id.as_str(),
                self.thread_id.as_str()
            );
        }
        self.queue.lock().await.push_back(mail);
        self.mailbox_tx.send_replace(());
        Ok(())
    }

    pub(crate) async fn subscribe_mailbox(&self) -> watch::Receiver<()> {
        let mut mailbox_watch = self.mailbox_tx.subscribe();
        if self.has_pending().await {
            mailbox_watch.mark_changed();
        }
        mailbox_watch
    }

    pub(crate) async fn has_pending(&self) -> bool {
        !self.queue.lock().await.is_empty()
    }

    pub(crate) async fn has_trigger_turn_pending(&self) -> bool {
        self.queue.lock().await.iter().any(|mail| mail.trigger_turn)
    }

    pub(crate) async fn drain(&self) -> Vec<InterAgentCommunication> {
        self.queue.lock().await.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TurnId;

    fn mail(recipient: &str, content: &str) -> InterAgentCommunication {
        InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_author"),
            author_path: "/root/child".to_string(),
            recipient_thread_id: ThreadId::new(recipient),
            recipient_path: "/root".to_string(),
            other_recipients: Vec::new(),
            content: content.to_string(),
            trigger_turn: false,
            source_turn_id: Some(TurnId::new("turn_child")),
            created_at: "2026-06-12T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn subscribe_marks_changed_when_mail_is_already_pending() {
        let inbox = ThreadInbox::new(ThreadId::new("thread_parent"));
        inbox
            .enqueue(mail("thread_parent", "done"))
            .await
            .expect("enqueue should accept matching recipient");

        let rx = inbox.subscribe_mailbox().await;

        assert!(
            rx.has_changed().expect("watch receiver should be open"),
            "subscribers created after mail arrives must not miss the wakeup"
        );
    }

    #[tokio::test]
    async fn enqueue_notifies_existing_subscribers() {
        let inbox = ThreadInbox::new(ThreadId::new("thread_parent"));
        let mut rx = inbox.subscribe_mailbox().await;

        inbox
            .enqueue(mail("thread_parent", "done"))
            .await
            .expect("enqueue should accept matching recipient");

        tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
            .await
            .expect("mailbox change should arrive")
            .expect("watch sender should stay open");
    }

    #[tokio::test]
    async fn drain_preserves_fifo_order() {
        let inbox = ThreadInbox::new(ThreadId::new("thread_parent"));
        inbox.enqueue(mail("thread_parent", "first")).await.unwrap();
        inbox
            .enqueue(mail("thread_parent", "second"))
            .await
            .unwrap();

        let drained = inbox.drain().await;

        assert_eq!(
            drained
                .iter()
                .map(|mail| mail.content.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(!inbox.has_pending().await);
    }

    #[tokio::test]
    async fn enqueue_rejects_wrong_recipient() {
        let inbox = ThreadInbox::new(ThreadId::new("thread_parent"));

        let err = inbox
            .enqueue(mail("thread_other", "wrong"))
            .await
            .expect_err("wrong recipient should be rejected");

        assert!(err.to_string().contains("does not match thread"));
    }

    #[tokio::test]
    async fn has_trigger_turn_pending_tracks_trigger_mail() {
        let inbox = ThreadInbox::new(ThreadId::new("thread_parent"));
        inbox.enqueue(mail("thread_parent", "plain")).await.unwrap();
        assert!(!inbox.has_trigger_turn_pending().await);

        let mut wake = mail("thread_parent", "wake");
        wake.trigger_turn = true;
        inbox.enqueue(wake).await.unwrap();

        assert!(inbox.has_trigger_turn_pending().await);
    }
}
