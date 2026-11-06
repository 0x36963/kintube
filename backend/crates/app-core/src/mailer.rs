use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart},
};

#[derive(Debug, Clone)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Outgoing email. `Memory` captures messages for tests instead of sending them.
#[derive(Clone)]
pub enum Mailer {
    Smtp {
        // Boxed: the transport is ~240 bytes and would bloat every clone of the enum.
        transport: Box<AsyncSmtpTransport<Tokio1Executor>>,
        from: Mailbox,
    },
    Memory(Arc<Mutex<Vec<Email>>>),
}

impl Mailer {
    pub fn smtp(url: &str, from: &str) -> Result<Self> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .context("invalid SMTP_URL")?
            .build();
        let from = from.parse().context("invalid EMAIL_FROM")?;
        Ok(Self::Smtp {
            transport: Box::new(transport),
            from,
        })
    }

    pub fn memory() -> Self {
        Self::Memory(Arc::default())
    }

    pub async fn send(&self, email: Email) -> Result<()> {
        match self {
            Self::Smtp { transport, from } => {
                let message = Message::builder()
                    .from(from.clone())
                    .to(email.to.parse().context("invalid recipient")?)
                    .subject(email.subject)
                    .multipart(MultiPart::alternative_plain_html(email.text, email.html))?;
                transport.send(message).await.context("SMTP send failed")?;
            }
            Self::Memory(sent) => sent.lock().expect("mailer lock poisoned").push(email),
        }
        Ok(())
    }

    /// Messages captured by a `Memory` mailer (empty for SMTP).
    pub fn sent(&self) -> Vec<Email> {
        match self {
            Self::Smtp { .. } => Vec::new(),
            Self::Memory(sent) => sent.lock().expect("mailer lock poisoned").clone(),
        }
    }
}
